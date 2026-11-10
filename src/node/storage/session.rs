use crate::common::error::{Error, Result};
use crate::common::types::{PaymentHash, Satoshis, SessionId};
use sqlx::{Pool, Sqlite};
use std::str::FromStr;
use uuid::Uuid;

/// Status of a persistent session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    PendingPayment,
    Active,
    Exhausted,
    Expired,
    Closed,
}

impl SessionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PendingPayment => "pending_payment",
            Self::Active => "active",
            Self::Exhausted => "exhausted",
            Self::Expired => "expired",
            Self::Closed => "closed",
        }
    }
}

impl FromStr for SessionStatus {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "pending_payment" => Ok(Self::PendingPayment),
            "active" => Ok(Self::Active),
            "exhausted" => Ok(Self::Exhausted),
            "expired" => Ok(Self::Expired),
            "closed" => Ok(Self::Closed),
            _ => Err(()),
        }
    }
}

/// Status of an individual session budget reservation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationStatus {
    Reserved,
    Finalized,
    Released,
    ReconciliationRequired,
}

impl ReservationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Finalized => "finalized",
            Self::Released => "released",
            Self::ReconciliationRequired => "reconciliation_required",
        }
    }
}

impl FromStr for ReservationStatus {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "reserved" => Ok(Self::Reserved),
            "finalized" => Ok(Self::Finalized),
            "released" => Ok(Self::Released),
            "reconciliation_required" => Ok(Self::ReconciliationRequired),
            _ => Err(()),
        }
    }
}

/// Record representing a persistent session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub id: SessionId,
    pub payment_hash: Option<PaymentHash>,
    pub total_budget_sats: Satoshis,
    pub remaining_sats: Satoshis,
    pub reserved_sats: Satoshis,
    pub spent_sats: Satoshis,
    pub status: SessionStatus,
    pub created_at: i64,
    pub expires_at: i64,
    pub updated_at: i64,
    pub closed_at: Option<i64>,
    pub closed_reason: Option<String>,
}

/// Outcome of attempting to close a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCloseResult {
    /// Session was closed successfully.
    Closed {
        total_budget_sats: Satoshis,
        remaining_sats: Satoshis,
        reserved_sats: Satoshis,
        spent_sats: Satoshis,
        closed_at: i64,
    },
    /// Session was already in closed state.
    AlreadyClosed { closed_at: Option<i64> },
    /// Session does not exist in storage.
    NotFound,
    /// Session has active reserved budget from in-flight requests.
    ActiveRequestsPending { reserved_sats: Satoshis },
}

/// Outcome of attempting to reserve budget from a session.
#[derive(Debug, PartialEq, Eq)]
pub enum SessionReservationResult {
    /// Successfully reserved budget.
    Reserved {
        reservation_id: String,
        remaining_sats: Satoshis,
    },
    /// Session was not found.
    NotFound,
    /// Session is expired.
    Expired { expires_at: i64 },
    /// Session is inactive (exhausted, pending_payment, or closed).
    Inactive { status: SessionStatus },
    /// Available budget is insufficient.
    InsufficientBudget {
        requested: Satoshis,
        available: Satoshis,
    },
}

/// Outcome of attempting to activate a pending-payment session.
#[derive(Debug, PartialEq, Eq)]
pub enum SessionActivationResult {
    /// Successfully transitioned from pending_payment to active and credited funding.
    Activated { total_budget: Satoshis },
    /// Session is already active or in another terminal status.
    AlreadyActive {
        status: SessionStatus,
        remaining: Satoshis,
    },
    /// Session record was not found in storage.
    NotFound,
    /// Session has already expired before activation.
    Expired { expires_at: i64 },
    /// The supplied payment hash does not match the session's recorded payment hash.
    PaymentHashMismatch,
}

type SessionRow = (
    String,
    Option<String>,
    i64,
    i64,
    i64,
    i64,
    String,
    i64,
    i64,
    i64,
    Option<i64>,
    Option<String>,
);

/// Durable session store providing atomic, persistent budget management.
#[derive(Clone, Debug)]
pub struct SessionStore {
    pool: Pool<Sqlite>,
}

impl SessionStore {
    pub fn new(pool: Pool<Sqlite>) -> Self {
        Self { pool }
    }

    /// Create a pending payment challenge for a session in persistent storage.
    ///
    /// Inserts a `sessions` record with `status = 'pending_payment'`, `remaining_sats = 0`.
    /// Zero ledger entries are recorded until settlement is confirmed.
    pub async fn create_pending_session(
        &self,
        session_id: &SessionId,
        payment_hash: Option<&PaymentHash>,
        total_budget: Satoshis,
        expires_at: i64,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();

        let insert_query = r#"
            INSERT INTO sessions (
                id, payment_hash, total_budget_sats, remaining_sats,
                reserved_sats, spent_sats, status, created_at, expires_at, updated_at
            ) VALUES (?1, ?2, ?3, 0, 0, 0, 'pending_payment', ?4, ?5, ?4)
        "#;

        sqlx::query(insert_query)
            .bind(session_id.0.to_string())
            .bind(payment_hash.map(|h| h.0.as_str()))
            .bind(total_budget.0 as i64)
            .bind(now)
            .bind(expires_at)
            .execute(&self.pool)
            .await
            .map_err(|e| match e {
                sqlx::Error::Database(ref db_err) if db_err.is_unique_violation() => {
                    Error::Internal(format!(
                        "Session or funding payment hash already exists: {}",
                        db_err
                    ))
                }
                _ => Error::Internal(format!("Failed to insert pending session: {}", e)),
            })?;

        Ok(())
    }

    /// Atomically insert a pending session and its funding invoice into SQLite within a single transaction.
    ///
    /// Preserves strict linkage between the session, the payment hash, and the invoice registry.
    pub async fn create_pending_session_with_invoice(
        &self,
        session_id: &SessionId,
        payment_hash: &PaymentHash,
        bolt11: &str,
        total_budget: Satoshis,
        expires_at: i64,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let mut tx = self.pool.begin().await.map_err(|e| {
            Error::Internal(format!(
                "Failed to begin transaction for pending session with invoice: {}",
                e
            ))
        })?;

        // 1. Insert pending session
        let insert_session = r#"
            INSERT INTO sessions (
                id, payment_hash, total_budget_sats, remaining_sats,
                reserved_sats, spent_sats, status, created_at, expires_at, updated_at
            ) VALUES (?1, ?2, ?3, 0, 0, 0, 'pending_payment', ?4, ?5, ?4)
        "#;

        sqlx::query(insert_session)
            .bind(session_id.0.to_string())
            .bind(&payment_hash.0)
            .bind(total_budget.0 as i64)
            .bind(now)
            .bind(expires_at)
            .execute(&mut *tx)
            .await
            .map_err(|e| match e {
                sqlx::Error::Database(ref db_err) if db_err.is_unique_violation() => {
                    Error::Internal(format!(
                        "Session or funding payment hash already exists: {}",
                        db_err
                    ))
                }
                _ => Error::Internal(format!("Failed to insert pending session: {}", e)),
            })?;

        // 2. Insert invoice record
        let insert_invoice = r#"
            INSERT INTO invoices (
                payment_hash, bolt11, amount_sats, purpose, session_id,
                status, created_at, expires_at, settled_at, updated_at
            ) VALUES (?1, ?2, ?3, 'session_funding', ?4, 'pending', ?5, ?6, NULL, ?5)
        "#;

        sqlx::query(insert_invoice)
            .bind(&payment_hash.0)
            .bind(bolt11)
            .bind(total_budget.0 as i64)
            .bind(session_id.0.to_string())
            .bind(now)
            .bind(expires_at)
            .execute(&mut *tx)
            .await
            .map_err(|e| match e {
                sqlx::Error::Database(ref db_err) if db_err.is_unique_violation() => {
                    Error::Internal(format!(
                        "Invoice with payment hash already exists: {}",
                        db_err
                    ))
                }
                _ => Error::Internal(format!("Failed to insert funding invoice: {}", e)),
            })?;

        tx.commit().await.map_err(|e| {
            Error::Internal(format!(
                "Failed to commit pending session with invoice: {}",
                e
            ))
        })?;

        Ok(())
    }

    /// Atomically activate a pending session upon verified payment settlement.
    ///
    /// Validates that the supplied `payment_hash` matches the session's recorded payment hash.
    /// Transitions `status = 'pending_payment'` -> `'active'`, credits `remaining_sats = total_budget_sats`,
    /// and records the append-only `session_funding` ledger entry exactly once.
    pub async fn activate_session(
        &self,
        session_id: &SessionId,
        payment_hash: &PaymentHash,
    ) -> Result<SessionActivationResult> {
        let now = chrono::Utc::now().timestamp();

        let mut conn = self.pool.acquire().await.map_err(|e| {
            Error::Internal(format!(
                "Failed to acquire connection for session activation: {}",
                e
            ))
        })?;

        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                Error::Internal(format!(
                    "Failed to begin immediate transaction for activation: {}",
                    e
                ))
            })?;

        let select_query = r#"
            SELECT payment_hash, total_budget_sats, remaining_sats, expires_at, status
            FROM sessions
            WHERE id = ?1
        "#;

        let row: Option<(Option<String>, i64, i64, i64, String)> = sqlx::query_as(select_query)
            .bind(session_id.0.to_string())
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to query session for activation: {}", e))
            })?;

        let (stored_payment_hash, total_budget, remaining, expires_at, status_str) = match row {
            Some(r) => r,
            None => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                return Ok(SessionActivationResult::NotFound);
            }
        };

        // Enforce explicit payment-hash binding:
        // Must match stored payment hash exactly; missing or mismatched hash is rejected.
        match stored_payment_hash {
            Some(ref h) if h.eq_ignore_ascii_case(&payment_hash.0) => {}
            _ => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                return Ok(SessionActivationResult::PaymentHashMismatch);
            }
        }

        let status = SessionStatus::from_str(&status_str)
            .map_err(|_| Error::Internal("Invalid session status in DB".to_string()))?;

        if status != SessionStatus::PendingPayment {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Ok(SessionActivationResult::AlreadyActive {
                status,
                remaining: Satoshis(remaining as u64),
            });
        }

        if now >= expires_at {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Ok(SessionActivationResult::Expired { expires_at });
        }

        // Transition session to active and credit budget
        let update_query = r#"
            UPDATE sessions
            SET status = 'active', remaining_sats = total_budget_sats, updated_at = ?1
            WHERE id = ?2 AND status = 'pending_payment'
        "#;

        let result = sqlx::query(update_query)
            .bind(now)
            .bind(session_id.0.to_string())
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Internal(format!("Failed to activate pending session: {}", e)))?;

        if result.rows_affected() == 0 {
            // Concurrent activation won
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Ok(SessionActivationResult::AlreadyActive {
                status: SessionStatus::Active,
                remaining: Satoshis(total_budget as u64),
            });
        }

        // Record initial session_funding ledger entry
        let entry_uuid = Uuid::new_v4().to_string();
        let ledger_query = r#"
            INSERT INTO accounting_entries (
                entry_uuid, session_id, reservation_id, entry_type, amount_sats, created_at, description
            ) VALUES (?1, ?2, NULL, 'session_funding', ?3, ?4, ?5)
        "#;

        let desc = format!("Initial session funding of {} sats", total_budget);
        sqlx::query(ledger_query)
            .bind(&entry_uuid)
            .bind(session_id.0.to_string())
            .bind(total_budget)
            .bind(now)
            .bind(desc)
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                Error::Internal(format!(
                    "Failed to record session activation funding ledger entry: {}",
                    e
                ))
            })?;

        sqlx::query("COMMIT")
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                Error::Internal(format!(
                    "Failed to commit session activation transaction: {}",
                    e
                ))
            })?;

        Ok(SessionActivationResult::Activated {
            total_budget: Satoshis(total_budget as u64),
        })
    }

    /// Create and fund a new session in persistent storage immediately.
    ///
    /// Inserts a `sessions` record and an append-only `accounting_entries` record for session funding.
    pub async fn create_session(
        &self,
        session_id: &SessionId,
        payment_hash: Option<&PaymentHash>,
        total_budget: Satoshis,
        expires_at: i64,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let mut tx = self.pool.begin().await.map_err(|e| {
            Error::Internal(format!(
                "Failed to begin session creation transaction: {}",
                e
            ))
        })?;

        // 1. Insert session record
        let insert_query = r#"
            INSERT INTO sessions (
                id, payment_hash, total_budget_sats, remaining_sats,
                reserved_sats, spent_sats, status, created_at, expires_at, updated_at
            ) VALUES (?1, ?2, ?3, ?3, 0, 0, 'active', ?4, ?5, ?4)
        "#;

        sqlx::query(insert_query)
            .bind(session_id.0.to_string())
            .bind(payment_hash.map(|h| h.0.as_str()))
            .bind(total_budget.0 as i64)
            .bind(now)
            .bind(expires_at)
            .execute(&mut *tx)
            .await
            .map_err(|e| match e {
                sqlx::Error::Database(ref db_err) if db_err.is_unique_violation() => {
                    Error::Internal(format!(
                        "Session or funding payment hash already exists: {}",
                        db_err
                    ))
                }
                _ => Error::Internal(format!("Failed to insert session: {}", e)),
            })?;

        // 2. Append-only ledger event: session_funding
        let entry_uuid = Uuid::new_v4().to_string();
        let ledger_query = r#"
            INSERT INTO accounting_entries (
                entry_uuid, session_id, reservation_id, entry_type, amount_sats, created_at, description
            ) VALUES (?1, ?2, NULL, 'session_funding', ?3, ?4, ?5)
        "#;

        let desc = format!("Initial session funding of {} sats", total_budget.0);
        sqlx::query(ledger_query)
            .bind(&entry_uuid)
            .bind(session_id.0.to_string())
            .bind(total_budget.0 as i64)
            .bind(now)
            .bind(desc)
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to record funding ledger entry: {}", e))
            })?;

        tx.commit().await.map_err(|e| {
            Error::Internal(format!(
                "Failed to commit session creation transaction: {}",
                e
            ))
        })?;

        Ok(())
    }

    /// Retrieve an existing session record by its ID.
    pub async fn get_session(&self, session_id: &SessionId) -> Result<Option<SessionRecord>> {
        let query = r#"
            SELECT id, payment_hash, total_budget_sats, remaining_sats,
                   reserved_sats, spent_sats, status, created_at, expires_at, updated_at,
                   closed_at, closed_reason
            FROM sessions
            WHERE id = ?1
        "#;

        let row: Option<SessionRow> = sqlx::query_as(query)
            .bind(session_id.0.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch session: {}", e)))?;

        match row {
            Some((
                id_str,
                hash_opt,
                total,
                remaining,
                reserved,
                spent,
                status_str,
                created_at,
                expires_at,
                updated_at,
                closed_at,
                closed_reason,
            )) => {
                let parsed_id = Uuid::parse_str(&id_str)
                    .map(SessionId)
                    .map_err(|e| Error::Internal(format!("Invalid UUID in database: {}", e)))?;
                let payment_hash = hash_opt.map(PaymentHash);
                let status = SessionStatus::from_str(&status_str)
                    .map_err(|_| Error::Internal("Invalid session status in DB".to_string()))?;

                Ok(Some(SessionRecord {
                    id: parsed_id,
                    payment_hash,
                    total_budget_sats: Satoshis(total as u64),
                    remaining_sats: Satoshis(remaining as u64),
                    reserved_sats: Satoshis(reserved as u64),
                    spent_sats: Satoshis(spent as u64),
                    status,
                    created_at,
                    expires_at,
                    updated_at,
                    closed_at,
                    closed_reason,
                }))
            }
            None => Ok(None),
        }
    }

    /// Atomically reserve a cost from a session budget within an immediate transaction.
    ///
    /// Validates session existence, active status, expiration, and available balance.
    /// Decreases `remaining_sats`, increases `reserved_sats`, updates status if exhausted,
    /// creates a `session_reservations` row, and records an append-only accounting event.
    pub async fn reserve_budget(
        &self,
        session_id: &SessionId,
        cost: Satoshis,
        reservation_id: &str,
    ) -> Result<SessionReservationResult> {
        let now = chrono::Utc::now().timestamp();

        // 1. Acquire dedicated connection from pool
        let mut conn = self.pool.acquire().await.map_err(|e| {
            Error::Internal(format!(
                "Failed to acquire connection for reservation: {}",
                e
            ))
        })?;

        // 2. Begin IMMEDIATE transaction to serialize concurrent writers and avoid lock escalation
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to begin immediate transaction: {}", e))
            })?;

        // 3. Fetch current session with row lock
        let select_query = r#"
            SELECT remaining_sats, expires_at, status
            FROM sessions
            WHERE id = ?1
        "#;

        let row: Option<(i64, i64, String)> = sqlx::query_as(select_query)
            .bind(session_id.0.to_string())
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to query session for reservation: {}", e))
            })?;

        let (remaining, expires_at, status_str) = match row {
            Some(vals) => vals,
            None => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                return Ok(SessionReservationResult::NotFound);
            }
        };

        let status = SessionStatus::from_str(&status_str)
            .map_err(|_| Error::Internal("Invalid session status in DB".to_string()))?;

        // 4. Check expiration
        if now >= expires_at {
            // Mark session expired in database
            let _ = sqlx::query("UPDATE sessions SET status = 'expired', updated_at = ?1 WHERE id = ?2 AND status != 'expired'")
                .bind(now)
                .bind(session_id.0.to_string())
                .execute(&mut *conn)
                .await;
            let _ = sqlx::query("COMMIT").execute(&mut *conn).await;
            return Ok(SessionReservationResult::Expired { expires_at });
        }

        // 5. Check status
        if status != SessionStatus::Active {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Ok(SessionReservationResult::Inactive { status });
        }

        // 6. Check budget
        if (remaining as u64) < cost.0 {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Ok(SessionReservationResult::InsufficientBudget {
                requested: cost,
                available: Satoshis(remaining as u64),
            });
        }

        // 7. Perform atomic balance mutation
        let new_remaining = (remaining as u64) - cost.0;
        let new_status = if new_remaining == 0 {
            "exhausted"
        } else {
            "active"
        };

        let update_query = r#"
            UPDATE sessions
            SET remaining_sats = remaining_sats - ?1,
                reserved_sats = reserved_sats + ?1,
                status = ?2,
                updated_at = ?3
            WHERE id = ?4 AND remaining_sats >= ?1 AND status = 'active'
        "#;

        let update_res = sqlx::query(update_query)
            .bind(cost.0 as i64)
            .bind(new_status)
            .bind(now)
            .bind(session_id.0.to_string())
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Internal(format!("Failed to update session balance: {}", e)))?;

        if update_res.rows_affected() == 0 {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            let fresh = self.get_session(session_id).await?;
            let avail = fresh.map(|s| s.remaining_sats).unwrap_or(Satoshis(0));
            return Ok(SessionReservationResult::InsufficientBudget {
                requested: cost,
                available: avail,
            });
        }

        // 8. Insert session reservation record
        let insert_res_query = r#"
            INSERT INTO session_reservations (
                id, session_id, reserved_sats, status, created_at
            ) VALUES (?1, ?2, ?3, 'reserved', ?4)
        "#;

        sqlx::query(insert_res_query)
            .bind(reservation_id)
            .bind(session_id.0.to_string())
            .bind(cost.0 as i64)
            .bind(now)
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Internal(format!("Failed to insert session reservation: {}", e)))?;

        // 9. Record accounting event: session_reservation
        let entry_uuid = Uuid::new_v4().to_string();
        let ledger_query = r#"
            INSERT INTO accounting_entries (
                entry_uuid, session_id, reservation_id, entry_type, amount_sats, created_at, description
            ) VALUES (?1, ?2, ?3, 'session_reservation', ?4, ?5, ?6)
        "#;

        let desc = format!(
            "Reserved {} sats for reservation {}",
            cost.0, reservation_id
        );
        sqlx::query(ledger_query)
            .bind(&entry_uuid)
            .bind(session_id.0.to_string())
            .bind(reservation_id)
            .bind(cost.0 as i64)
            .bind(now)
            .bind(desc)
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to record reservation ledger entry: {}", e))
            })?;

        sqlx::query("COMMIT")
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to commit reservation transaction: {}", e))
            })?;

        Ok(SessionReservationResult::Reserved {
            reservation_id: reservation_id.to_string(),
            remaining_sats: Satoshis(new_remaining),
        })
    }

    /// Finalize a successful reservation.
    ///
    /// Decreases `reserved_sats`, increases `spent_sats`, transitions reservation to `finalized`,
    /// and records an append-only `session_debit` accounting entry.
    ///
    /// Idempotent: Attempting to finalize an already finalized or non-reserved reservation fails with an error.
    pub async fn finalize_reservation(&self, reservation_id: &str) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let mut conn = self.pool.acquire().await.map_err(|e| {
            Error::Internal(format!(
                "Failed to acquire connection for finalization: {}",
                e
            ))
        })?;

        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to begin immediate transaction: {}", e))
            })?;

        // 1. Fetch reservation and verify status == 'reserved'
        let fetch_query = r#"
            SELECT session_id, reserved_sats, status
            FROM session_reservations
            WHERE id = ?1
        "#;

        let row: Option<(String, i64, String)> = sqlx::query_as(fetch_query)
            .bind(reservation_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch reservation: {}", e)))?;

        let (session_id_str, reserved_sats, status_str) = match row {
            Some(r) => r,
            None => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                return Err(Error::Internal(format!(
                    "Reservation '{}' not found",
                    reservation_id
                )));
            }
        };

        if status_str != "reserved" {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Err(Error::Internal(format!(
                "Cannot finalize reservation '{}': current status is '{}'",
                reservation_id, status_str
            )));
        }

        // 2. Mark reservation finalized
        let update_res = sqlx::query(
            "UPDATE session_reservations SET status = 'finalized', finalized_at = ?1 WHERE id = ?2 AND status = 'reserved'",
        )
        .bind(now)
        .bind(reservation_id)
        .execute(&mut *conn)
        .await
        .map_err(|e| Error::Internal(format!("Failed to finalize reservation record: {}", e)))?;

        if update_res.rows_affected() == 0 {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Err(Error::Internal(format!(
                "Concurrent modification during finalization of reservation '{}'",
                reservation_id
            )));
        }

        // 3. Update session balance: decrement reserved, increment spent
        let update_session = r#"
            UPDATE sessions
            SET reserved_sats = reserved_sats - ?1,
                spent_sats = spent_sats + ?1,
                updated_at = ?2
            WHERE id = ?3 AND reserved_sats >= ?1
        "#;

        sqlx::query(update_session)
            .bind(reserved_sats)
            .bind(now)
            .bind(&session_id_str)
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to update session spent balance: {}", e))
            })?;

        // 4. Record accounting event: session_debit
        let entry_uuid = Uuid::new_v4().to_string();
        let ledger_query = r#"
            INSERT INTO accounting_entries (
                entry_uuid, session_id, reservation_id, entry_type, amount_sats, created_at, description
            ) VALUES (?1, ?2, ?3, 'session_debit', ?4, ?5, ?6)
        "#;

        let desc = format!(
            "Finalized debit of {} sats for reservation {}",
            reserved_sats, reservation_id
        );
        sqlx::query(ledger_query)
            .bind(&entry_uuid)
            .bind(&session_id_str)
            .bind(reservation_id)
            .bind(reserved_sats)
            .bind(now)
            .bind(desc)
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Internal(format!("Failed to record debit ledger entry: {}", e)))?;

        sqlx::query("COMMIT")
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to commit finalization transaction: {}", e))
            })?;

        Ok(())
    }

    /// Release an unconsumed reservation (e.g. on confirmed pre-generation failure).
    ///
    /// Decreases `reserved_sats`, restores `remaining_sats`, marks reservation as `released`,
    /// and records an append-only `session_reservation_release` entry.
    ///
    /// Idempotent: Attempting to release an already finalized or released reservation fails with an error.
    pub async fn release_reservation(
        &self,
        reservation_id: &str,
        failure_reason: &str,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let mut conn = self.pool.acquire().await.map_err(|e| {
            Error::Internal(format!("Failed to acquire connection for release: {}", e))
        })?;

        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to begin immediate transaction: {}", e))
            })?;

        // 1. Fetch reservation
        let fetch_query = r#"
            SELECT session_id, reserved_sats, status
            FROM session_reservations
            WHERE id = ?1
        "#;

        let row: Option<(String, i64, String)> = sqlx::query_as(fetch_query)
            .bind(reservation_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch reservation: {}", e)))?;

        let (session_id_str, reserved_sats, status_str) = match row {
            Some(r) => r,
            None => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                return Err(Error::Internal(format!(
                    "Reservation '{}' not found",
                    reservation_id
                )));
            }
        };

        if status_str != "reserved" {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Err(Error::Internal(format!(
                "Cannot release reservation '{}': current status is '{}'",
                reservation_id, status_str
            )));
        }

        // 2. Mark reservation released
        let update_res = sqlx::query(
            "UPDATE session_reservations SET status = 'released', finalized_at = ?1, failure_reason = ?2 WHERE id = ?3 AND status = 'reserved'",
        )
        .bind(now)
        .bind(failure_reason)
        .bind(reservation_id)
        .execute(&mut *conn)
        .await
        .map_err(|e| Error::Internal(format!("Failed to release reservation record: {}", e)))?;

        if update_res.rows_affected() == 0 {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Err(Error::Internal(format!(
                "Concurrent modification during release of reservation '{}'",
                reservation_id
            )));
        }

        // 3. Restore session balance: decrement reserved, restore remaining, update status to active
        let update_session = r#"
            UPDATE sessions
            SET reserved_sats = reserved_sats - ?1,
                remaining_sats = remaining_sats + ?1,
                status = 'active',
                updated_at = ?2
            WHERE id = ?3 AND reserved_sats >= ?1
        "#;

        sqlx::query(update_session)
            .bind(reserved_sats)
            .bind(now)
            .bind(&session_id_str)
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Internal(format!("Failed to restore session balance: {}", e)))?;

        // 4. Record accounting event: session_reservation_release
        let entry_uuid = Uuid::new_v4().to_string();
        let ledger_query = r#"
            INSERT INTO accounting_entries (
                entry_uuid, session_id, reservation_id, entry_type, amount_sats, created_at, description
            ) VALUES (?1, ?2, ?3, 'session_reservation_release', ?4, ?5, ?6)
        "#;

        let desc = format!(
            "Released reservation {} ({} sats) due to: {}",
            reservation_id, reserved_sats, failure_reason
        );
        sqlx::query(ledger_query)
            .bind(&entry_uuid)
            .bind(&session_id_str)
            .bind(reservation_id)
            .bind(reserved_sats)
            .bind(now)
            .bind(desc)
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to record release ledger entry: {}", e))
            })?;

        sqlx::query("COMMIT")
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Internal(format!("Failed to commit release transaction: {}", e)))?;

        Ok(())
    }

    /// Flag an ambiguous reservation as requiring manual/operator reconciliation.
    ///
    /// Leaves the reservation intact and un-refunded.
    pub async fn mark_reconciliation_required(
        &self,
        reservation_id: &str,
        reason: &str,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let query = r#"
            UPDATE session_reservations
            SET status = 'reconciliation_required',
                failure_reason = ?1,
                finalized_at = ?2
            WHERE id = ?3 AND status = 'reserved'
        "#;

        let res = sqlx::query(query)
            .bind(reason)
            .bind(now)
            .bind(reservation_id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                Error::Internal(format!(
                    "Failed to mark reservation as reconciliation required: {}",
                    e
                ))
            })?;

        if res.rows_affected() == 0 {
            return Err(Error::Internal(format!(
                "Reservation '{}' not found or not in reserved state",
                reservation_id
            )));
        }

        Ok(())
    }

    /// Startup crash recovery:
    ///
    /// Scans for reservations left in the `reserved` state on boot.
    /// - If explicitly marked or known definitely undispatched: safe to release.
    /// - Ambiguous in-flight reservations: marked `reconciliation_required` without releasing balance.
    pub async fn recover_in_flight_reservations(
        &self,
        definitely_undispatched_ids: &[String],
    ) -> Result<usize> {
        let mut recovered_count = 0;

        // 1. Release reservations known definitely not dispatched
        for res_id in definitely_undispatched_ids {
            if self
                .release_reservation(res_id, "server_crash_prior_to_dispatch")
                .await
                .is_ok()
            {
                recovered_count += 1;
            }
        }

        // 2. Mark remaining ambiguous reservations as requiring reconciliation
        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "UPDATE session_reservations SET status = 'reconciliation_required', failure_reason = 'unresolved_crash_during_execution', finalized_at = ?1 WHERE status = 'reserved'",
        )
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("Failed to mark ambiguous reservations: {}", e)))?;

        Ok(recovered_count)
    }

    /// Atomically close a session in persistent storage.
    ///
    /// Lifecycle semantics:
    /// - If `status == 'closed'`, returns `SessionCloseResult::AlreadyClosed`.
    /// - If `reserved_sats > 0`, in-flight requests or un-reconciled reservations exist:
    ///   rejects closure with `SessionCloseResult::ActiveRequestsPending { reserved_sats }`.
    /// - If `status == 'pending_payment'`, transitions directly to `'closed'`, zeroes remaining balance,
    ///   and preserves the `(status = 'closed' AND remaining_sats + reserved_sats + spent_sats = total_budget_sats)` constraint
    ///   by setting `spent_sats = total_budget_sats` (forfeited budget).
    /// - If `status == 'active'` or `'expired'` or `'exhausted'`:
    ///   transitions to `'closed'`, sets `closed_at = now`, records `closed_reason`,
    ///   transfers any `remaining_sats` to `spent_sats` (forfeiture / closure settlement),
    ///   and appends an immutable audit event to `accounting_entries`.
    pub async fn close_session(
        &self,
        session_id: &SessionId,
        reason: Option<&str>,
    ) -> Result<SessionCloseResult> {
        let now = chrono::Utc::now().timestamp();

        let mut conn = self.pool.acquire().await.map_err(|e| {
            Error::Internal(format!(
                "Failed to acquire connection for session closure: {}",
                e
            ))
        })?;

        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                Error::Internal(format!(
                    "Failed to begin immediate transaction for closure: {}",
                    e
                ))
            })?;

        let select_query = r#"
            SELECT total_budget_sats, remaining_sats, reserved_sats, spent_sats, status, closed_at
            FROM sessions
            WHERE id = ?1
        "#;

        let row: Option<(i64, i64, i64, i64, String, Option<i64>)> = sqlx::query_as(select_query)
            .bind(session_id.0.to_string())
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| Error::Internal(format!("Failed to query session for closure: {}", e)))?;

        let (total, remaining, reserved, spent, status_str, closed_at_opt) = match row {
            Some(r) => r,
            None => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                return Ok(SessionCloseResult::NotFound);
            }
        };

        let status = SessionStatus::from_str(&status_str)
            .map_err(|_| Error::Internal("Invalid session status in DB".to_string()))?;

        // 1. Idempotency: If already closed, return AlreadyClosed immediately
        if status == SessionStatus::Closed {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Ok(SessionCloseResult::AlreadyClosed {
                closed_at: closed_at_opt,
            });
        }

        // 2. Active in-flight requests guard
        if reserved > 0 {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Ok(SessionCloseResult::ActiveRequestsPending {
                reserved_sats: Satoshis(reserved as u64),
            });
        }

        // 3. Balance accounting adjustments:
        // For active/expired/exhausted sessions, remaining satoshis are forfeited/settled upon closure,
        // transferring to spent_sats to satisfy: remaining_sats + reserved_sats + spent_sats = total_budget_sats.
        // For pending_payment sessions, no funds were deposited (remaining = 0, spent = 0, reserved = 0),
        // so total_budget is zeroed to satisfy: 0 + 0 + 0 = 0 under the non-pending constraint.
        let (final_total, final_spent) = if status == SessionStatus::PendingPayment {
            (0, 0)
        } else {
            (total, spent + remaining)
        };
        let closure_reason = reason.unwrap_or("voluntary_caller_close");

        let update_query = r#"
            UPDATE sessions
            SET status = 'closed',
                total_budget_sats = ?1,
                remaining_sats = 0,
                spent_sats = ?2,
                closed_at = ?3,
                closed_reason = ?4,
                updated_at = ?3
            WHERE id = ?5 AND status != 'closed' AND reserved_sats = 0
        "#;

        let result = sqlx::query(update_query)
            .bind(final_total)
            .bind(final_spent)
            .bind(now)
            .bind(closure_reason)
            .bind(session_id.0.to_string())
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Internal(format!("Failed to update session to closed: {}", e)))?;

        if result.rows_affected() == 0 {
            // Concurrent closure won or reserved_sats changed concurrently
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            let fresh = self.get_session(session_id).await?;
            if let Some(s) = fresh {
                if s.status == SessionStatus::Closed {
                    return Ok(SessionCloseResult::AlreadyClosed {
                        closed_at: s.closed_at,
                    });
                } else if s.reserved_sats.0 > 0 {
                    return Ok(SessionCloseResult::ActiveRequestsPending {
                        reserved_sats: s.reserved_sats,
                    });
                }
            }
            return Ok(SessionCloseResult::NotFound);
        }

        // 4. Record accounting event if funds were active
        if status != SessionStatus::PendingPayment && remaining > 0 {
            let entry_uuid = Uuid::new_v4().to_string();
            let desc = format!("Session closed: {} unused sats forfeited", remaining);
            let ledger_query = r#"
                INSERT INTO accounting_entries (
                    entry_uuid, session_id, reservation_id, entry_type, amount_sats, created_at, description
                ) VALUES (?1, ?2, NULL, 'session_debit', ?3, ?4, ?5)
            "#;

            sqlx::query(ledger_query)
                .bind(&entry_uuid)
                .bind(session_id.0.to_string())
                .bind(remaining)
                .bind(now)
                .bind(desc)
                .execute(&mut *conn)
                .await
                .map_err(|e| {
                    Error::Internal(format!("Failed to record closure ledger entry: {}", e))
                })?;
        }

        sqlx::query("COMMIT")
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                Error::Internal(format!(
                    "Failed to commit session closure transaction: {}",
                    e
                ))
            })?;

        Ok(SessionCloseResult::Closed {
            total_budget_sats: Satoshis(final_total as u64),
            remaining_sats: Satoshis(0),
            reserved_sats: Satoshis(0),
            spent_sats: Satoshis(final_spent as u64),
            closed_at: now,
        })
    }

    /// Read accounting ledger entries for a session to audit satoshi movements.
    pub async fn get_accounting_entries(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<(String, String, i64, i64, String)>> {
        let query = r#"
            SELECT entry_uuid, entry_type, amount_sats, created_at, description
            FROM accounting_entries
            WHERE session_id = ?1
            ORDER BY id ASC
        "#;

        let rows: Vec<(String, String, i64, i64, String)> = sqlx::query_as(query)
            .bind(session_id.0.to_string())
            .fetch_all(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch accounting entries: {}", e)))?;

        Ok(rows)
    }
}
