use crate::common::error::{Error, Result};
use crate::common::types::PaymentHash;
use sqlx::{Pool, Sqlite};

/// Represents the state of a payment authorization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentAuthState {
    Reserved,
    Settled,
    Consumed,
}

impl PaymentAuthState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Settled => "settled",
            Self::Consumed => "consumed",
        }
    }
}

impl std::str::FromStr for PaymentAuthState {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "reserved" => Ok(Self::Reserved),
            "settled" => Ok(Self::Settled),
            "consumed" => Ok(Self::Consumed),
            _ => Err(()),
        }
    }
}

/// Result of an atomic reservation attempt.
#[derive(Debug, PartialEq, Eq)]
pub enum ReservationResult {
    /// Successfully reserved for this request.
    Reserved,
    /// Already consumed or reserved by another request (replay detected).
    AlreadyUsed { state: String, reserved_at: i64 },
}

/// Durable payment authorization store providing atomic, single-use reservation semantics.
#[derive(Clone, Debug)]
pub struct PaymentStore {
    pool: Pool<Sqlite>,
}

impl PaymentStore {
    pub fn new(pool: Pool<Sqlite>) -> Self {
        Self { pool }
    }

    /// Attempt to reserve a pay-per-request payment hash atomically.
    ///
    /// Returns `Ok(ReservationResult::Reserved)` if this is the first submission.
    /// Returns `Ok(ReservationResult::AlreadyUsed)` if the hash was previously recorded.
    /// Returns `Err(Error)` if SQLite is unavailable or encounters a non-constraint error.
    pub async fn reserve_payment_hash(
        &self,
        payment_hash: &PaymentHash,
        request_id: &str,
        expires_at: u64,
        idempotency_key: Option<&str>,
    ) -> Result<ReservationResult> {
        let now = chrono::Utc::now().timestamp();

        let query = r#"
            INSERT INTO payment_authorizations (
                payment_hash,
                auth_type,
                state,
                idempotency_key,
                request_id,
                created_at,
                reserved_at,
                settled_at,
                expires_at
            ) VALUES (?1, 'pay_per_request', 'reserved', ?2, ?3, ?4, ?4, NULL, ?5)
        "#;

        let insert_res = sqlx::query(query)
            .bind(&payment_hash.0)
            .bind(idempotency_key)
            .bind(request_id)
            .bind(now)
            .bind(expires_at as i64)
            .execute(&self.pool)
            .await;

        match insert_res {
            Ok(_) => Ok(ReservationResult::Reserved),
            Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
                // Fetch existing record to provide diagnostic detail
                let existing: Option<(String, i64)> = sqlx::query_as(
                    "SELECT state, reserved_at FROM payment_authorizations WHERE payment_hash = ?1",
                )
                .bind(&payment_hash.0)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| {
                    Error::Internal(format!("Failed to query payment authorization: {}", e))
                })?;

                if let Some((state, reserved_at)) = existing {
                    Ok(ReservationResult::AlreadyUsed { state, reserved_at })
                } else {
                    Ok(ReservationResult::AlreadyUsed {
                        state: "consumed".to_string(),
                        reserved_at: now,
                    })
                }
            }
            Err(e) => Err(Error::Internal(format!(
                "Database error reserving payment hash: {}",
                e
            ))),
        }
    }

    /// Mark an existing reserved payment authorization as settled and consumed.
    pub async fn mark_settled_and_consumed(&self, payment_hash: &PaymentHash) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "UPDATE payment_authorizations SET state = 'consumed', settled_at = ?1 WHERE payment_hash = ?2",
        )
        .bind(now)
        .bind(&payment_hash.0)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("Failed to update payment authorization: {}", e)))?;

        Ok(())
    }

    /// Look up the authorization state of a given payment hash.
    pub async fn get_authorization_state(
        &self,
        payment_hash: &PaymentHash,
    ) -> Result<Option<String>> {
        let state: Option<String> =
            sqlx::query_scalar("SELECT state FROM payment_authorizations WHERE payment_hash = ?1")
                .bind(&payment_hash.0)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| {
                    Error::Internal(format!("Failed to fetch authorization state: {}", e))
                })?;

        Ok(state)
    }
}
