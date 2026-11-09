use crate::common::error::{Error, Result};
use crate::common::types::{PaymentHash, Satoshis, SessionId};
use sqlx::{Pool, Sqlite};
use std::str::FromStr;
use uuid::Uuid;

/// Purpose of an invoice issued by the node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvoicePurpose {
    PayPerRequest,
    SessionFunding,
}

impl InvoicePurpose {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PayPerRequest => "pay_per_request",
            Self::SessionFunding => "session_funding",
        }
    }
}

impl FromStr for InvoicePurpose {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "pay_per_request" => Ok(Self::PayPerRequest),
            "session_funding" => Ok(Self::SessionFunding),
            _ => Err(()),
        }
    }
}

/// Durable settlement status of an invoice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvoiceStatus {
    Pending,
    Settled,
    Expired,
    Cancelled,
}

impl InvoiceStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Settled => "settled",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
        }
    }
}

impl FromStr for InvoiceStatus {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "settled" => Ok(Self::Settled),
            "expired" => Ok(Self::Expired),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(()),
        }
    }
}

/// Durable record of an invoice created by the node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvoiceRecord {
    pub payment_hash: PaymentHash,
    pub bolt11: String,
    pub amount_sats: Satoshis,
    pub purpose: InvoicePurpose,
    pub session_id: Option<SessionId>,
    pub status: InvoiceStatus,
    pub created_at: i64,
    pub expires_at: i64,
    pub settled_at: Option<i64>,
    pub updated_at: i64,
}

type InvoiceRow = (
    String,
    String,
    i64,
    String,
    Option<String>,
    String,
    i64,
    i64,
    Option<i64>,
    i64,
);

/// Result of an invoice settlement transition attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvoiceSettlementResult {
    /// The invoice was in `pending` status and successfully transitioned to `settled`.
    Settled,
    /// The invoice was already recorded as `settled` (idempotent no-op).
    AlreadySettled,
    /// The invoice was marked `expired`, but Lightning backend confirmed settlement (state conflict).
    ConflictExpired,
    /// The invoice was marked `cancelled`, but Lightning backend confirmed settlement (state conflict).
    ConflictCancelled,
    /// The invoice was not found in the database.
    NotFound,
}

/// Durable store for PayGuard invoice tracking.
#[derive(Clone, Debug)]
pub struct InvoiceStore {
    pool: Pool<Sqlite>,
}

impl InvoiceStore {
    pub fn new(pool: Pool<Sqlite>) -> Self {
        Self { pool }
    }

    /// Persist a new invoice in the durable registry before an HTTP 402 challenge is emitted.
    pub async fn create_invoice(
        &self,
        payment_hash: &PaymentHash,
        bolt11: &str,
        amount_sats: Satoshis,
        purpose: InvoicePurpose,
        session_id: Option<&SessionId>,
        expires_at: i64,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let query = r#"
            INSERT INTO invoices (
                payment_hash, bolt11, amount_sats, purpose, session_id,
                status, created_at, expires_at, settled_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7, NULL, ?6)
        "#;

        sqlx::query(query)
            .bind(&payment_hash.0)
            .bind(bolt11)
            .bind(amount_sats.0 as i64)
            .bind(purpose.as_str())
            .bind(session_id.map(|s| s.0.to_string()))
            .bind(now)
            .bind(expires_at)
            .execute(&self.pool)
            .await
            .map_err(|e| match e {
                sqlx::Error::Database(ref db_err) if db_err.is_unique_violation() => {
                    Error::Internal(format!("Invoice payment hash already exists: {}", db_err))
                }
                _ => Error::Internal(format!("Failed to insert invoice: {}", e)),
            })?;

        Ok(())
    }

    /// Retrieve an invoice by its payment hash.
    pub async fn get_invoice(&self, payment_hash: &PaymentHash) -> Result<Option<InvoiceRecord>> {
        let query = r#"
            SELECT payment_hash, bolt11, amount_sats, purpose, session_id,
                   status, created_at, expires_at, settled_at, updated_at
            FROM invoices
            WHERE payment_hash = ?1
        "#;

        let row: Option<InvoiceRow> = sqlx::query_as(query)
            .bind(&payment_hash.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch invoice: {}", e)))?;

        match row {
            Some((
                hash,
                bolt11,
                amount,
                purpose_str,
                session_opt,
                status_str,
                created_at,
                expires_at,
                settled_at,
                updated_at,
            )) => {
                let purpose = InvoicePurpose::from_str(&purpose_str)
                    .map_err(|_| Error::Internal("Invalid invoice purpose in DB".to_string()))?;
                let status = InvoiceStatus::from_str(&status_str)
                    .map_err(|_| Error::Internal("Invalid invoice status in DB".to_string()))?;
                let session_id =
                    match session_opt {
                        Some(s) => Some(Uuid::parse_str(&s).map(SessionId).map_err(|e| {
                            Error::Internal(format!("Invalid UUID in invoice: {}", e))
                        })?),
                        None => None,
                    };

                Ok(Some(InvoiceRecord {
                    payment_hash: PaymentHash(hash),
                    bolt11,
                    amount_sats: Satoshis(amount as u64),
                    purpose,
                    session_id,
                    status,
                    created_at,
                    expires_at,
                    settled_at,
                    updated_at,
                }))
            }
            None => Ok(None),
        }
    }

    /// Mark an invoice as settled in the durable registry with explicit state transition semantics.
    ///
    /// - If `pending`: atomically transitions to `settled`, recording `settled_at`. Returns `Ok(InvoiceSettlementResult::Settled)`.
    /// - If already `settled`: returns `Ok(InvoiceSettlementResult::AlreadySettled)` (idempotent).
    /// - If `expired`: returns `Ok(InvoiceSettlementResult::ConflictExpired)` without overwriting stored state.
    /// - If `cancelled`: returns `Ok(InvoiceSettlementResult::ConflictCancelled)` without overwriting stored state.
    /// - If not found: returns `Ok(InvoiceSettlementResult::NotFound)`.
    pub async fn mark_settled(
        &self,
        payment_hash: &PaymentHash,
        settled_at: i64,
    ) -> Result<InvoiceSettlementResult> {
        let now = chrono::Utc::now().timestamp();
        let mut conn = self.pool.acquire().await.map_err(|e| {
            Error::Internal(format!(
                "Failed to acquire connection for mark_settled: {}",
                e
            ))
        })?;

        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                Error::Internal(format!(
                    "Failed to begin transaction for mark_settled: {}",
                    e
                ))
            })?;

        let status_row: Option<(String, Option<i64>)> =
            sqlx::query_as("SELECT status, settled_at FROM invoices WHERE payment_hash = ?1")
                .bind(&payment_hash.0)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| Error::Internal(format!("Failed to query invoice status: {}", e)))?;

        let (status_str, _existing_settled_at) = match status_row {
            Some(row) => row,
            None => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                return Ok(InvoiceSettlementResult::NotFound);
            }
        };

        match status_str.as_str() {
            "pending" => {
                let update_query = r#"
                    UPDATE invoices
                    SET status = 'settled', settled_at = ?1, updated_at = ?2
                    WHERE payment_hash = ?3 AND status = 'pending'
                "#;
                sqlx::query(update_query)
                    .bind(settled_at)
                    .bind(now)
                    .bind(&payment_hash.0)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| {
                        Error::Internal(format!("Failed to update invoice to settled: {}", e))
                    })?;

                sqlx::query("COMMIT")
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| {
                        Error::Internal(format!("Failed to commit mark_settled: {}", e))
                    })?;

                Ok(InvoiceSettlementResult::Settled)
            }
            "settled" => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Ok(InvoiceSettlementResult::AlreadySettled)
            }
            "expired" => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                tracing::warn!(
                    payment_hash = %payment_hash.0,
                    "Lightning confirmed payment for invoice previously marked expired"
                );
                Ok(InvoiceSettlementResult::ConflictExpired)
            }
            "cancelled" => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                tracing::warn!(
                    payment_hash = %payment_hash.0,
                    "Lightning confirmed payment for invoice previously marked cancelled"
                );
                Ok(InvoiceSettlementResult::ConflictCancelled)
            }
            _ => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Err(Error::Internal(format!(
                    "Unknown invoice status in DB: {}",
                    status_str
                )))
            }
        }
    }

    /// Mark an invoice as expired if it has passed its expiration timestamp.
    pub async fn mark_expired(&self, payment_hash: &PaymentHash) -> Result<bool> {
        let now = chrono::Utc::now().timestamp();
        let query = r#"
            UPDATE invoices
            SET status = 'expired', updated_at = ?1
            WHERE payment_hash = ?2 AND status = 'pending' AND ?1 >= expires_at
        "#;

        let res = sqlx::query(query)
            .bind(now)
            .bind(&payment_hash.0)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("Failed to mark invoice expired: {}", e)))?;

        Ok(res.rows_affected() > 0)
    }
}
