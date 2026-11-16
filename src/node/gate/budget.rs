use crate::common::error::{Error, Result};
use crate::common::types::{Satoshis, SessionId};
use crate::node::gate::macaroon::{Caveat, Macaroon};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;

/// HTTP Header name used to report remaining session budget to callers and agents.
pub const HEADER_REMAINING_BUDGET: &str = "x-satsgrid-remaining-budget-sats";

/// A thread-safe, lock-free spend tracker for an active multi-step session.
#[derive(Debug, Clone)]
pub struct SessionBudget {
    pub total_sats: Satoshis,
    remaining_sats: Arc<AtomicU64>,
}

impl SessionBudget {
    pub fn new(total_sats: Satoshis) -> Self {
        Self {
            total_sats,
            remaining_sats: Arc::new(AtomicU64::new(total_sats.0)),
        }
    }

    /// Atomically debit a cost in Satoshis from this budget.
    ///
    /// Uses lock-free Compare-And-Swap (CAS). Returns the remaining Satoshis on success,
    /// or `Error::BudgetExhausted` if the balance is insufficient.
    pub fn debit(&self, cost: Satoshis) -> Result<Satoshis> {
        let mut current = self.remaining_sats.load(Ordering::SeqCst);
        loop {
            if current < cost.0 {
                return Err(Error::BudgetExhausted(format!(
                    "Request cost {} sats exceeds remaining budget of {} sats",
                    cost.0, current
                )));
            }
            match self.remaining_sats.compare_exchange_weak(
                current,
                current - cost.0,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return Ok(Satoshis(current - cost.0)),
                Err(actual) => current = actual,
            }
        }
    }

    /// Read the currently remaining Satoshis without modifying state.
    pub fn remaining(&self) -> Satoshis {
        Satoshis(self.remaining_sats.load(Ordering::SeqCst))
    }
}

/// Manager tracking active session budgets across multiple concurrent requests,
/// backed by durable SQLite storage when a `SessionStore` is provided, or an in-memory map otherwise.
#[derive(Clone, Default)]
pub struct SessionBudgetManager {
    sessions: Arc<RwLock<HashMap<String, SessionBudget>>>,
    pending_sessions: Arc<RwLock<HashMap<String, Satoshis>>>,
    store: Option<crate::node::storage::session::SessionStore>,
}

impl SessionBudgetManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            pending_sessions: Arc::new(RwLock::new(HashMap::new())),
            store: None,
        }
    }

    /// Construct a SessionBudgetManager backed by durable SQLite storage.
    pub fn with_store(store: crate::node::storage::session::SessionStore) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            pending_sessions: Arc::new(RwLock::new(HashMap::new())),
            store: Some(store),
        }
    }

    /// Open or register a new session with an allocated budget limit.
    pub async fn open_session(&self, session_id: &SessionId, total_budget: Satoshis) {
        if let Some(store) = &self.store {
            let default_expiry = chrono::Utc::now().timestamp() + 86400 * 30; // 30 days default
            let _ = store
                .create_session(session_id, None, total_budget, default_expiry)
                .await;
        }

        let mut lock = self.sessions.write().await;
        lock.insert(session_id.0.to_string(), SessionBudget::new(total_budget));
    }

    /// Open a persistent session with explicit funding payment hash and expiry.
    pub async fn open_session_with_details(
        &self,
        session_id: &SessionId,
        payment_hash: Option<&crate::common::types::PaymentHash>,
        total_budget: Satoshis,
        expires_at: i64,
    ) -> Result<()> {
        if let Some(store) = &self.store {
            store
                .create_session(session_id, payment_hash, total_budget, expires_at)
                .await?;
        }

        let mut lock = self.sessions.write().await;
        lock.insert(session_id.0.to_string(), SessionBudget::new(total_budget));
        Ok(())
    }

    /// Register a pending-payment session challenge.
    ///
    /// Persists a session with `status = 'pending_payment'` in SQLite when backed by storage,
    /// without crediting any in-memory spend cache or inserting accounting ledger entries.
    pub async fn create_pending_session(
        &self,
        session_id: &SessionId,
        payment_hash: Option<&crate::common::types::PaymentHash>,
        total_budget: Satoshis,
        expires_at: i64,
    ) -> Result<()> {
        if let Some(store) = &self.store {
            store
                .create_pending_session(session_id, payment_hash, total_budget, expires_at)
                .await?;
        } else {
            // Track pending session budget for ephemeral in-memory mode
            let mut lock = self.pending_sessions.write().await;
            lock.insert(session_id.0.to_string(), total_budget);
        }
        // Do NOT populate in-memory spend cache (sessions) while pending!
        Ok(())
    }

    /// Lazily activate a pending session upon verified payment settlement.
    ///
    /// Validates that the supplied `payment_hash` matches the session's recorded payment hash in SQLite.
    /// Delegates durable activation to SQLite. If the session was pending, it transitions to active
    /// and inserts the single session_funding ledger entry. Synchronizes in-memory budget cache only
    /// after database commit.
    pub async fn activate_session_if_pending(
        &self,
        session_id: &SessionId,
        payment_hash: &crate::common::types::PaymentHash,
    ) -> Result<Satoshis> {
        if let Some(store) = &self.store {
            let act_result = store.activate_session(session_id, payment_hash).await?;
            match act_result {
                crate::node::storage::SessionActivationResult::Activated { total_budget } => {
                    let mut lock = self.sessions.write().await;
                    lock.insert(session_id.0.to_string(), SessionBudget::new(total_budget));
                    Ok(total_budget)
                }
                crate::node::storage::SessionActivationResult::PaymentHashMismatch => {
                    Err(Error::VerificationFailed(format!(
                        "Payment hash mismatch for session '{}'",
                        session_id.0
                    )))
                }
                crate::node::storage::SessionActivationResult::AlreadyActive {
                    status,
                    remaining,
                } => {
                    match status {
                        crate::node::storage::SessionStatus::Active => {
                            // Ensure in-memory cache has the active session
                            let mut lock = self.sessions.write().await;
                            lock.entry(session_id.0.to_string())
                                .or_insert_with(|| SessionBudget {
                                    total_sats: remaining,
                                    remaining_sats: Arc::new(AtomicU64::new(remaining.0)),
                                });
                            Ok(remaining)
                        }
                        crate::node::storage::SessionStatus::Exhausted => {
                            Err(Error::BudgetExhausted(format!(
                                "Session '{}' is exhausted",
                                session_id.0
                            )))
                        }
                        crate::node::storage::SessionStatus::Expired => {
                            Err(Error::VerificationFailed(format!(
                                "Session '{}' has expired",
                                session_id.0
                            )))
                        }
                        crate::node::storage::SessionStatus::Closed => {
                            Err(Error::VerificationFailed(format!(
                                "Session '{}' is closed",
                                session_id.0
                            )))
                        }
                        crate::node::storage::SessionStatus::PendingPayment => {
                            Err(Error::VerificationFailed(format!(
                                "Session '{}' is still pending payment",
                                session_id.0
                            )))
                        }
                    }
                }
                crate::node::storage::SessionActivationResult::NotFound => Err(
                    Error::VerificationFailed(format!("Session '{}' not found", session_id.0)),
                ),
                crate::node::storage::SessionActivationResult::Expired { .. } => {
                    Err(Error::VerificationFailed(format!(
                        "Session '{}' has expired before activation",
                        session_id.0
                    )))
                }
            }
        } else {
            // In-memory fallback (e.g. unit tests without DB)
            // Check if already active
            {
                let lock = self.sessions.read().await;
                if let Some(b) = lock.get(&session_id.0.to_string()) {
                    return Ok(b.remaining());
                }
            }

            // Check if pending, and if so activate
            let mut pending_lock = self.pending_sessions.write().await;
            if let Some(total_budget) = pending_lock.remove(&session_id.0.to_string()) {
                let mut sessions_lock = self.sessions.write().await;
                sessions_lock.insert(session_id.0.to_string(), SessionBudget::new(total_budget));
                Ok(total_budget)
            } else {
                Err(Error::VerificationFailed(format!(
                    "Session '{}' not found",
                    session_id.0
                )))
            }
        }
    }

    /// Look up an active session budget by session ID.
    pub async fn get_session(&self, session_id: &SessionId) -> Option<SessionBudget> {
        if let Some(store) = &self.store {
            if let Ok(Some(record)) = store.get_session(session_id).await {
                return Some(SessionBudget {
                    total_sats: record.total_budget_sats,
                    remaining_sats: Arc::new(AtomicU64::new(record.remaining_sats.0)),
                });
            }
            return None;
        }

        let lock = self.sessions.read().await;
        lock.get(&session_id.0.to_string()).cloned()
    }

    /// Atomically reserve a cost from an active session, returning the reservation ID and remaining balance.
    pub async fn reserve_session(
        &self,
        session_id: &SessionId,
        cost: Satoshis,
        reservation_id: &str,
    ) -> Result<Satoshis> {
        if let Some(store) = &self.store {
            match store
                .reserve_budget(session_id, cost, reservation_id)
                .await?
            {
                crate::node::storage::SessionReservationResult::Reserved {
                    remaining_sats, ..
                } => {
                    // Sync in-memory view
                    let mut lock = self.sessions.write().await;
                    if let Some(b) = lock.get_mut(&session_id.0.to_string()) {
                        b.remaining_sats.store(remaining_sats.0, Ordering::SeqCst);
                    }
                    return Ok(remaining_sats);
                }
                crate::node::storage::SessionReservationResult::NotFound => {
                    return Err(Error::VerificationFailed(format!(
                        "Session '{}' not found",
                        session_id.0
                    )));
                }
                crate::node::storage::SessionReservationResult::Expired { .. } => {
                    return Err(Error::VerificationFailed(format!(
                        "Session '{}' has expired",
                        session_id.0
                    )));
                }
                crate::node::storage::SessionReservationResult::Inactive { status } => {
                    return Err(Error::VerificationFailed(format!(
                        "Session '{}' is not active (status: {})",
                        session_id.0,
                        status.as_str()
                    )));
                }
                crate::node::storage::SessionReservationResult::InsufficientBudget {
                    requested,
                    available,
                } => {
                    return Err(Error::BudgetExhausted(format!(
                        "Request cost {} sats exceeds remaining budget of {} sats",
                        requested.0, available.0
                    )));
                }
            }
        }

        // Ephemeral in-memory fallback
        let session = {
            let lock = self.sessions.read().await;
            lock.get(&session_id.0.to_string())
                .cloned()
                .ok_or_else(|| {
                    Error::VerificationFailed(format!(
                        "Session '{}' not found or expired",
                        session_id.0
                    ))
                })?
        };

        session.debit(cost)
    }

    /// Finalize an active reservation.
    pub async fn finalize_reservation(&self, reservation_id: &str) -> Result<()> {
        if let Some(store) = &self.store {
            store.finalize_reservation(reservation_id).await?;
        }
        Ok(())
    }

    /// Release an active reservation (e.g. pre-generation failure).
    pub async fn release_reservation(&self, reservation_id: &str, reason: &str) -> Result<()> {
        if let Some(store) = &self.store {
            store.release_reservation(reservation_id, reason).await?;
        }
        Ok(())
    }

    /// Debit an active session, returning the new remaining balance.
    ///
    /// For backward compatibility with tests and callers expecting direct debit semantics:
    /// executes an atomic reserve followed immediately by finalization.
    pub async fn debit_session(&self, session_id: &SessionId, cost: Satoshis) -> Result<Satoshis> {
        if self.store.is_some() {
            let res_id = uuid::Uuid::new_v4().to_string();
            let remaining = self.reserve_session(session_id, cost, &res_id).await?;
            self.finalize_reservation(&res_id).await?;
            return Ok(remaining);
        }

        let session = self.get_session(session_id).await.ok_or_else(|| {
            Error::VerificationFailed(format!("Session '{}' not found or expired", session_id.0))
        })?;

        session.debit(cost)
    }

    /// Inspect a macaroon's caveats to extract any declared session ID and budget limit.
    pub fn extract_session_caveats(macaroon: &Macaroon) -> (Option<SessionId>, Option<Satoshis>) {
        let mut session_id = None;
        let mut budget = None;

        for raw_caveat in &macaroon.caveats {
            match Caveat::parse(raw_caveat) {
                Some(Caveat::Session(s)) => {
                    if let Ok(uuid) = uuid::Uuid::parse_str(&s) {
                        session_id = Some(SessionId(uuid));
                    }
                }
                Some(Caveat::Budget(b)) => {
                    budget = Some(Satoshis(b));
                }
                _ => {}
            }
        }

        (session_id, budget)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_session_budget_debit_success() {
        let budget = SessionBudget::new(Satoshis(100));
        assert_eq!(budget.remaining(), Satoshis(100));

        let rem = budget.debit(Satoshis(30)).expect("debit should succeed");
        assert_eq!(rem, Satoshis(70));
        assert_eq!(budget.remaining(), Satoshis(70));

        let rem2 = budget.debit(Satoshis(70)).expect("debit should succeed");
        assert_eq!(rem2, Satoshis(0));
        assert_eq!(budget.remaining(), Satoshis(0));
    }

    #[test]
    fn test_session_budget_exhaustion_errors() {
        let budget = SessionBudget::new(Satoshis(50));
        let result = budget.debit(Satoshis(60));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, Error::BudgetExhausted(_)));
        assert_eq!(budget.remaining(), Satoshis(50)); // Balance remains untouched
    }

    #[tokio::test]
    async fn test_session_budget_manager_open_and_debit() {
        let manager = SessionBudgetManager::new();
        let session_id = SessionId(Uuid::new_v4());

        manager.open_session(&session_id, Satoshis(200)).await;

        let rem = manager
            .debit_session(&session_id, Satoshis(75))
            .await
            .expect("should debit");
        assert_eq!(rem, Satoshis(125));

        let current = manager.get_session(&session_id).await.unwrap().remaining();
        assert_eq!(current, Satoshis(125));
    }

    #[tokio::test]
    async fn test_session_budget_manager_nonexistent_session_errors() {
        let manager = SessionBudgetManager::new();
        let session_id = SessionId(Uuid::new_v4());

        let result = manager.debit_session(&session_id, Satoshis(10)).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_concurrent_debits_atomic_safety() {
        let budget = Arc::new(SessionBudget::new(Satoshis(1000)));
        let mut handles = Vec::new();

        // 10 concurrent tasks each debiting 100 sats
        for _ in 0..10 {
            let b = budget.clone();
            handles.push(tokio::spawn(async move {
                b.debit(Satoshis(100)).expect("each debit should succeed");
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        assert_eq!(budget.remaining(), Satoshis(0));

        // 11th debit must fail
        assert!(budget.debit(Satoshis(1)).is_err());
    }

    #[test]
    fn test_extract_session_caveats_from_macaroon() {
        let session_uuid = Uuid::new_v4();
        let macaroon = Macaroon {
            location: "node".to_string(),
            identifier: "hash".to_string(),
            caveats: vec![
                format!("session = {}", session_uuid),
                "budget = 500".to_string(),
                "time < 9999999999".to_string(),
            ],
            signature: "sig".to_string(),
        };

        let (extracted_id, extracted_budget) =
            SessionBudgetManager::extract_session_caveats(&macaroon);

        assert_eq!(extracted_id, Some(SessionId(session_uuid)));
        assert_eq!(extracted_budget, Some(Satoshis(500)));
    }
}
