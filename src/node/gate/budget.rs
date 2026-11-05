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

/// In-memory manager tracking active session budgets across multiple concurrent requests.
#[derive(Clone, Default)]
pub struct SessionBudgetManager {
    sessions: Arc<RwLock<HashMap<String, SessionBudget>>>,
}

impl SessionBudgetManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Open or register a new session with an allocated budget limit.
    pub async fn open_session(&self, session_id: &SessionId, total_budget: Satoshis) {
        let mut lock = self.sessions.write().await;
        lock.insert(session_id.0.to_string(), SessionBudget::new(total_budget));
    }

    /// Look up an active session budget by session ID.
    pub async fn get_session(&self, session_id: &SessionId) -> Option<SessionBudget> {
        let lock = self.sessions.read().await;
        lock.get(&session_id.0.to_string()).cloned()
    }

    /// Debit an active session, returning the new remaining balance.
    pub async fn debit_session(&self, session_id: &SessionId, cost: Satoshis) -> Result<Satoshis> {
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
