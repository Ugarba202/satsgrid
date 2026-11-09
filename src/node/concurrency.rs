use crate::common::error::{Error, Result};
use crate::common::types::SessionId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Internal session entry tracking active permit holders and in-flight acquirers.
#[derive(Debug)]
pub struct SessionEntry {
    semaphore: Arc<Semaphore>,
    leases: usize,
    last_active: Instant,
}

impl SessionEntry {
    /// Decrement the lease count with explicit underflow detection and invariant enforcement.
    ///
    /// Updates `last_active` if and only if the lease count cleanly transitions to zero.
    pub fn decrement_lease(&mut self) -> bool {
        if self.leases == 0 {
            tracing::error!(
                "Session concurrency lease underflow detected: attempted to decrement entry with 0 active leases"
            );
            return false;
        }

        self.leases -= 1;
        if self.leases == 0 {
            self.last_active = Instant::now();
        }
        true
    }
}

/// Thread-safe registry governing per-session concurrency.
pub struct SessionConcurrencyRegistry {
    max_per_session: usize,
    entries: Mutex<HashMap<SessionId, Arc<Mutex<SessionEntry>>>>,
}

impl SessionConcurrencyRegistry {
    pub fn new(max_per_session: usize) -> Self {
        Self {
            max_per_session,
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn max_per_session(&self) -> usize {
        self.max_per_session
    }

    /// Try to acquire a permit for the specified session without blocking.
    ///
    /// The lease count is incremented under registry protection *before* releasing the lock.
    /// If permit acquisition fails, the lease is immediately decremented.
    pub fn try_acquire(&self, session_id: &SessionId) -> Result<SessionPermitGuard> {
        let (entry_arc, sem) = {
            let mut map = self.entries.lock().map_err(|e| {
                Error::Internal(format!("Session registry map lock poisoned: {}", e))
            })?;
            let entry_arc = map
                .entry(session_id.clone())
                .or_insert_with(|| {
                    Arc::new(Mutex::new(SessionEntry {
                        semaphore: Arc::new(Semaphore::new(self.max_per_session)),
                        leases: 0,
                        last_active: Instant::now(),
                    }))
                })
                .clone();

            let sem = {
                let mut entry = entry_arc
                    .lock()
                    .map_err(|e| Error::Internal(format!("Session entry lock poisoned: {}", e)))?;
                entry.leases += 1;
                entry.semaphore.clone()
            };
            (entry_arc, sem)
        };

        match sem.try_acquire_owned() {
            Ok(permit) => Ok(SessionPermitGuard {
                permit: Some(permit),
                entry: entry_arc,
            }),
            Err(_) => {
                if let Ok(mut entry) = entry_arc.lock() {
                    entry.decrement_lease();
                }
                Err(Error::ConcurrencyLimitExceeded(format!(
                    "Session concurrency limit exceeded for session '{}'",
                    session_id.0
                )))
            }
        }
    }

    /// Clean up idle entries where no leases are held and the idle TTL has expired.
    pub fn cleanup_idle_entries(&self, idle_ttl: Duration) {
        let now = Instant::now();
        if let Ok(mut map) = self.entries.lock() {
            map.retain(|_session_id, entry_arc| {
                if let Ok(entry) = entry_arc.lock() {
                    if entry.leases > 0 {
                        return true;
                    }
                    now.duration_since(entry.last_active) < idle_ttl
                } else {
                    false
                }
            });
        }
    }

    /// Diagnostic count of active session entries currently tracked in the registry.
    pub fn entry_count(&self) -> usize {
        self.entries.lock().map(|m| m.len()).unwrap_or(0)
    }

    /// Diagnostic check of active leases for a session.
    pub fn lease_count_for(&self, session_id: &SessionId) -> Option<usize> {
        let map = self.entries.lock().ok()?;
        map.get(session_id)
            .and_then(|e| e.lock().ok().map(|entry| entry.leases))
    }
}

/// RAII guard ensuring the session semaphore permit is released *before* the registry lease count is decremented.
#[derive(Debug)]
pub struct SessionPermitGuard {
    permit: Option<OwnedSemaphorePermit>,
    entry: Arc<Mutex<SessionEntry>>,
}

impl Drop for SessionPermitGuard {
    fn drop(&mut self) {
        // Crucial ordering: explicitly drop the semaphore permit first to return the permit
        // to the semaphore before decrementing the registry lease count.
        drop(self.permit.take());

        if let Ok(mut entry) = self.entry.lock() {
            entry.decrement_lease();
        }
    }
}

/// Unified permit managing both global and per-session capacity.
#[derive(Debug)]
pub struct ConcurrencyPermit {
    pub _global: OwnedSemaphorePermit,
    pub _session: Option<SessionPermitGuard>,
}

/// Global Concurrency Manager governing node-wide and per-session admission.
pub struct ConcurrencyManager {
    global_semaphore: Arc<Semaphore>,
    session_registry: SessionConcurrencyRegistry,
    admission_timeout: Duration,
}

impl ConcurrencyManager {
    pub fn new(
        max_concurrent_requests: usize,
        max_concurrent_per_session: usize,
        admission_timeout_secs: u64,
    ) -> Self {
        Self {
            global_semaphore: Arc::new(Semaphore::new(max_concurrent_requests)),
            session_registry: SessionConcurrencyRegistry::new(max_concurrent_per_session),
            admission_timeout: Duration::from_secs(admission_timeout_secs),
        }
    }

    pub fn global_available_permits(&self) -> usize {
        self.global_semaphore.available_permits()
    }

    pub fn session_registry(&self) -> &SessionConcurrencyRegistry {
        &self.session_registry
    }

    pub fn admission_timeout(&self) -> Duration {
        self.admission_timeout
    }

    /// Attempt to acquire all required permits without blocking.
    ///
    /// If either the global or session acquisition fails, any partially acquired permit
    /// is immediately dropped so no capacity is held.
    pub fn try_acquire_all_now(&self, session_id: Option<&SessionId>) -> Result<ConcurrencyPermit> {
        let global_permit = match self.global_semaphore.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                return Err(Error::ServerOverloaded(
                    "Node capacity exceeded. All available inference slots are currently occupied."
                        .to_string(),
                ));
            }
        };

        if let Some(sid) = session_id {
            match self.session_registry.try_acquire(sid) {
                Ok(session_guard) => Ok(ConcurrencyPermit {
                    _global: global_permit,
                    _session: Some(session_guard),
                }),
                Err(e) => {
                    // Drop global permit immediately on session rejection!
                    drop(global_permit);
                    Err(e)
                }
            }
        } else {
            Ok(ConcurrencyPermit {
                _global: global_permit,
                _session: None,
            })
        }
    }

    /// Acquire required concurrency permits honoring the configured admission timeout.
    ///
    /// If `admission_timeout` is 0 (fail-fast), this is genuinely non-blocking.
    /// If `admission_timeout` is > 0, retries with non-hoarding bounded exponential backoff
    /// until both permits are acquired or the deadline expires.
    pub async fn acquire(&self, session_id: Option<&SessionId>) -> Result<ConcurrencyPermit> {
        if self.admission_timeout.is_zero() {
            return self.try_acquire_all_now(session_id);
        }

        let deadline = tokio::time::Instant::now() + self.admission_timeout;
        let mut backoff = Duration::from_millis(5);
        const MAX_BACKOFF: Duration = Duration::from_millis(25);

        loop {
            match self.try_acquire_all_now(session_id) {
                Ok(permit) => return Ok(permit),
                Err(err) => {
                    let now = tokio::time::Instant::now();
                    if now >= deadline {
                        return Err(err);
                    }

                    let remaining = deadline - now;
                    let sleep_duration = backoff.min(remaining);

                    tokio::time::sleep(sleep_duration).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_global_permit_acquisition_and_release() {
        let manager = ConcurrencyManager::new(2, 1, 0);
        assert_eq!(manager.global_available_permits(), 2);

        let permit1 = manager.try_acquire_all_now(None).unwrap();
        assert_eq!(manager.global_available_permits(), 1);

        {
            let permit2 = manager.try_acquire_all_now(None).unwrap();
            assert_eq!(manager.global_available_permits(), 0);
            drop(permit2);
        }

        assert_eq!(manager.global_available_permits(), 1);
        drop(permit1);
        assert_eq!(manager.global_available_permits(), 2);
    }

    #[tokio::test]
    async fn test_global_capacity_rejection() {
        let manager = ConcurrencyManager::new(1, 1, 0);
        let _permit = manager.try_acquire_all_now(None).unwrap();

        let err = manager.try_acquire_all_now(None).unwrap_err();
        match err {
            Error::ServerOverloaded(msg) => {
                assert!(msg.contains("Node capacity exceeded"));
            }
            other => panic!("Expected ServerOverloaded error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_per_session_capacity_enforcement() {
        let manager = ConcurrencyManager::new(4, 1, 0);
        let session_id = SessionId(Uuid::new_v4());

        let permit1 = manager.try_acquire_all_now(Some(&session_id)).unwrap();
        assert!(permit1._session.is_some());

        // Second request for same session must be rejected because max_concurrent_per_session is 1
        let err = manager.try_acquire_all_now(Some(&session_id)).unwrap_err();
        match err {
            Error::ConcurrencyLimitExceeded(msg) => {
                assert!(msg.contains("Session concurrency limit exceeded"));
            }
            other => panic!("Expected ConcurrencyLimitExceeded error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_different_session_isolation() {
        let manager = ConcurrencyManager::new(4, 1, 0);
        let session_a = SessionId(Uuid::new_v4());
        let session_b = SessionId(Uuid::new_v4());

        let permit_a = manager.try_acquire_all_now(Some(&session_a)).unwrap();
        let permit_b = manager.try_acquire_all_now(Some(&session_b)).unwrap();

        assert_eq!(manager.global_available_permits(), 2);
        drop(permit_a);
        drop(permit_b);
        assert_eq!(manager.global_available_permits(), 4);
    }

    #[tokio::test]
    async fn test_partial_acquisition_cleanup() {
        // Global capacity = 2, session limit = 1
        let manager = ConcurrencyManager::new(2, 1, 0);
        let session_id = SessionId(Uuid::new_v4());

        // Occupy the 1 session permit
        let _permit1 = manager.try_acquire_all_now(Some(&session_id)).unwrap();
        assert_eq!(manager.global_available_permits(), 1);

        // Attempt second request for same session: global permit would succeed, but session will fail.
        // Must immediately drop the global permit!
        let err = manager.try_acquire_all_now(Some(&session_id)).unwrap_err();
        assert!(matches!(err, Error::ConcurrencyLimitExceeded(_)));

        // Global capacity must STILL be 1 (not 0), proving global permit was dropped on session rejection
        assert_eq!(
            manager.global_available_permits(),
            1,
            "Global permit must be dropped immediately upon session failure"
        );
    }

    #[tokio::test]
    async fn test_immediate_acquisition_in_fail_fast_mode() {
        let manager = ConcurrencyManager::new(2, 2, 0);
        let start = Instant::now();
        let permit = manager.acquire(None).await.unwrap();
        assert!(start.elapsed() < Duration::from_millis(5));
        drop(permit);
    }

    #[tokio::test]
    async fn test_positive_timeout_acquisition_after_capacity_becomes_available() {
        let manager = Arc::new(ConcurrencyManager::new(1, 1, 2)); // 2 second admission timeout
        let permit = manager.try_acquire_all_now(None).unwrap();

        let mgr_clone = manager.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            drop(permit); // Release capacity
        });

        let start = Instant::now();
        let acquired = mgr_clone.acquire(None).await;
        assert!(acquired.is_ok());
        assert!(start.elapsed() >= Duration::from_millis(40));
    }

    #[tokio::test]
    async fn test_positive_timeout_expiry() {
        let manager = Arc::new(ConcurrencyManager::new(1, 1, 1)); // 1 second admission timeout
        let _permit = manager.try_acquire_all_now(None).unwrap();

        let start = Instant::now();
        let err = manager.acquire(None).await.unwrap_err();
        assert!(matches!(err, Error::ServerOverloaded(_)));
        assert!(start.elapsed() >= Duration::from_millis(900));
    }

    #[tokio::test]
    async fn test_cancellation_during_retry_sleep() {
        let manager = Arc::new(ConcurrencyManager::new(1, 1, 5));
        let _permit = manager.try_acquire_all_now(None).unwrap();

        let mgr_clone = manager.clone();
        let handle = tokio::spawn(async move { mgr_clone.acquire(None).await });

        tokio::time::sleep(Duration::from_millis(50)).await;
        // Abort waiting task during retry backoff
        handle.abort();
        let _ = handle.await;

        // Ensure no permits or leases were leaked
        assert_eq!(manager.global_available_permits(), 0);
    }

    #[tokio::test]
    async fn test_concurrent_session_entry_creation() {
        let registry = Arc::new(SessionConcurrencyRegistry::new(4));
        let session_id = SessionId(Uuid::new_v4());

        let mut handles = Vec::new();
        for _ in 0..20 {
            let reg = registry.clone();
            let sid = session_id.clone();
            handles.push(tokio::spawn(async move { reg.try_acquire(&sid) }));
        }

        let mut success = 0;
        let mut rejected = 0;
        for h in handles {
            match h.await.unwrap() {
                Ok(_guard) => success += 1,
                Err(Error::ConcurrencyLimitExceeded(_)) => rejected += 1,
                Err(e) => panic!("Unexpected error: {:?}", e),
            }
        }

        // Exactly 4 should succeed, 16 rejected
        assert_eq!(success, 4);
        assert_eq!(rejected, 16);
        // Crucial: exactly 1 entry in the registry map
        assert_eq!(registry.entry_count(), 1);
    }

    #[tokio::test]
    async fn test_registry_cleanup_with_active_leases() {
        let registry = SessionConcurrencyRegistry::new(2);
        let session_id = SessionId(Uuid::new_v4());

        let guard = registry.try_acquire(&session_id).unwrap();
        assert_eq!(registry.lease_count_for(&session_id), Some(1));

        // Attempt cleanup with 0 TTL - entry has lease = 1, so MUST NOT be evicted
        registry.cleanup_idle_entries(Duration::from_secs(0));
        assert_eq!(registry.entry_count(), 1);

        drop(guard);
        assert_eq!(registry.lease_count_for(&session_id), Some(0));

        // Now with lease = 0, cleanup with 0 TTL must evict it
        registry.cleanup_idle_entries(Duration::from_secs(0));
        assert_eq!(registry.entry_count(), 0);
    }

    #[tokio::test]
    async fn test_registry_eviction_after_idle_ttl() {
        let registry = SessionConcurrencyRegistry::new(2);
        let session_id = SessionId(Uuid::new_v4());

        let guard = registry.try_acquire(&session_id).unwrap();
        drop(guard);

        // Not evicted if TTL is in future (e.g. 1 hour)
        registry.cleanup_idle_entries(Duration::from_secs(3600));
        assert_eq!(registry.entry_count(), 1);

        // Evicted if TTL is 0
        registry.cleanup_idle_entries(Duration::from_secs(0));
        assert_eq!(registry.entry_count(), 0);
    }

    #[tokio::test]
    async fn test_reacquisition_after_eviction() {
        let registry = SessionConcurrencyRegistry::new(2);
        let session_id = SessionId(Uuid::new_v4());

        let guard1 = registry.try_acquire(&session_id).unwrap();
        drop(guard1);

        registry.cleanup_idle_entries(Duration::from_secs(0));
        assert_eq!(registry.entry_count(), 0);

        // Reacquire for same session
        let guard2 = registry.try_acquire(&session_id).unwrap();
        assert_eq!(registry.entry_count(), 1);
        drop(guard2);
    }

    #[tokio::test]
    async fn test_session_permit_release_before_lease_decrement() {
        let registry = Arc::new(SessionConcurrencyRegistry::new(1));
        let session_id = SessionId(Uuid::new_v4());

        let guard = registry.try_acquire(&session_id).unwrap();
        assert_eq!(registry.lease_count_for(&session_id), Some(1));

        // Dropping guard drops semaphore permit first, then decrements lease
        drop(guard);
        assert_eq!(registry.lease_count_for(&session_id), Some(0));

        // Immediately reacquiring must succeed because permit was freed
        let guard2 = registry.try_acquire(&session_id);
        assert!(guard2.is_ok());
    }

    #[test]
    fn test_checked_lease_decrement_and_underflow_detection() {
        let mut entry = SessionEntry {
            semaphore: Arc::new(Semaphore::new(1)),
            leases: 2,
            last_active: Instant::now() - Duration::from_secs(60),
        };

        let initial_last_active = entry.last_active;

        // Decrement 1: leases becomes 1, last_active should NOT be updated
        assert!(entry.decrement_lease());
        assert_eq!(entry.leases, 1);
        assert_eq!(entry.last_active, initial_last_active);

        // Decrement 2: leases becomes 0, last_active MUST be updated
        assert!(entry.decrement_lease());
        assert_eq!(entry.leases, 0);
        assert!(entry.last_active > initial_last_active);

        // Attempted decrement from 0: must return false and NOT underflow
        let zero_last_active = entry.last_active;
        assert!(!entry.decrement_lease());
        assert_eq!(entry.leases, 0);
        assert_eq!(entry.last_active, zero_last_active);
    }

    #[tokio::test]
    async fn test_failed_acquisition_rollback_decrements_lease_exactly_once() {
        let registry = SessionConcurrencyRegistry::new(1);
        let session_id = SessionId(Uuid::new_v4());

        // Fill capacity
        let _guard = registry.try_acquire(&session_id).unwrap();
        assert_eq!(registry.lease_count_for(&session_id), Some(1));

        // Second acquisition fails, must increment then immediately decrement lease
        let err = registry.try_acquire(&session_id).unwrap_err();
        assert!(matches!(err, Error::ConcurrencyLimitExceeded(_)));

        // Lease count must still be exactly 1
        assert_eq!(registry.lease_count_for(&session_id), Some(1));
    }

    #[test]
    fn test_zero_concurrency_limits_rejected_by_config_validation() {
        let mut config = crate::config::schema::NodeConfig::default();
        config.concurrency.max_concurrent_requests = 0;
        let err = config.validate().unwrap_err();
        assert!(err
            .to_string()
            .contains("concurrency.max_concurrent_requests must be greater than 0"));

        let mut config = crate::config::schema::NodeConfig::default();
        config.concurrency.max_concurrent_per_session = 0;
        let err = config.validate().unwrap_err();
        assert!(err
            .to_string()
            .contains("concurrency.max_concurrent_per_session must be greater than 0"));
    }

    #[test]
    fn test_per_session_limit_greater_than_global_limit_rejected() {
        let mut config = crate::config::schema::NodeConfig::default();
        config.concurrency.max_concurrent_requests = 2;
        config.concurrency.max_concurrent_per_session = 5;
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains(
            "concurrency.max_concurrent_per_session cannot exceed max_concurrent_requests"
        ));
    }
}
