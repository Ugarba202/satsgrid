use crate::common::error::Result;
use crate::node::lightning::LightningBackend;
use crate::node::storage::reconciliation::{ReconciliationItemType, ReconciliationStore};
use crate::node::storage::session::SessionStore;
use crate::node::storage::Database;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Configuration for the background recovery and reconciliation worker.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub poll_interval: Duration,
    pub batch_size: i64,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(30),
            batch_size: 50,
        }
    }
}

/// Statistics and health metrics reported by the reconciliation worker.
#[derive(Debug, Clone, Default)]
pub struct WorkerStats {
    pub last_scan_timestamp: Option<i64>,
    pub scans_completed: u64,
    pub invoices_checked: u64,
    pub invoices_settled: u64,
    pub invoices_expired: u64,
    pub items_reconciled: u64,
    pub last_error: Option<String>,
}

/// Durable, safe, idempotent recovery and reconciliation worker for SatsGrid.
pub struct ReconciliationWorker {
    db: Arc<Database>,
    lightning: Arc<dyn LightningBackend>,
    config: WorkerConfig,
    stats: Arc<tokio::sync::RwLock<WorkerStats>>,
}

impl ReconciliationWorker {
    pub fn new(
        db: Arc<Database>,
        lightning: Arc<dyn LightningBackend>,
        config: WorkerConfig,
    ) -> Self {
        Self {
            db,
            lightning,
            config,
            stats: Arc::new(tokio::sync::RwLock::new(WorkerStats::default())),
        }
    }

    /// Access live worker statistics.
    pub async fn stats(&self) -> WorkerStats {
        self.stats.read().await.clone()
    }

    /// Run a single bounded reconciliation pass over pending invoices and ambiguous requests.
    pub async fn run_reconciliation_pass(&self) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let invoice_store = self.db.invoice_store();
        let session_store = self.db.session_store();
        let recon_store = self.db.reconciliation_store();

        // 1. Fetch bounded batch of pending invoices
        let pending_invoices = invoice_store
            .get_pending_invoices(self.config.batch_size)
            .await?;

        for inv in pending_invoices {
            // Check expiry first if not settled
            if now >= inv.expires_at {
                // Before marking expired, verify authoritative backend settlement status
                match self.lightning.is_invoice_settled(&inv.payment_hash).await {
                    Ok(true) => {
                        self.handle_settled_invoice(
                            &inv,
                            &invoice_store,
                            &session_store,
                            &recon_store,
                        )
                        .await?;
                    }
                    Ok(false) => {
                        if invoice_store.mark_expired(&inv.payment_hash).await? {
                            let mut stats = self.stats.write().await;
                            stats.invoices_expired += 1;
                        }
                    }
                    Err(e) => {
                        // Transient backend error: do NOT mark expired; record retry item
                        tracing::warn!(
                            payment_hash = %inv.payment_hash.0,
                            "Failed to query backend settlement for expiring invoice: {}", e
                        );
                        let mut stats = self.stats.write().await;
                        stats.last_error = Some(e.to_string());
                    }
                }
                continue;
            }

            // Normal pending invoice check
            match self.lightning.is_invoice_settled(&inv.payment_hash).await {
                Ok(true) => {
                    self.handle_settled_invoice(&inv, &invoice_store, &session_store, &recon_store)
                        .await?;
                }
                Ok(false) => {
                    // Still open and within expiry: no action needed
                }
                Err(e) => {
                    tracing::debug!(
                        payment_hash = %inv.payment_hash.0,
                        "Backend lookup failed for pending invoice: {}", e
                    );
                    let mut stats = self.stats.write().await;
                    stats.last_error = Some(e.to_string());
                }
            }
        }

        // 2. Fetch and process eligible retry reconciliation items (excluding items requiring manual operator review like late session funding)
        let eligible_items = recon_store
            .get_eligible_retry_items(self.config.batch_size)
            .await?;

        for item in eligible_items {
            // Late session funding cannot be auto-resolved; it requires operator inspection
            if item.item_type == ReconciliationItemType::SessionLateFunding {
                continue;
            }

            if let Some(ref hash) = item.payment_hash {
                match self.lightning.is_invoice_settled(hash).await {
                    Ok(true) => {
                        let _ = invoice_store.mark_settled(hash, now).await;
                        let _ = recon_store.mark_resolved(&item.id).await;
                        let mut stats = self.stats.write().await;
                        stats.items_reconciled += 1;
                    }
                    Ok(false) => {
                        // Reschedule with backoff
                        let attempts = item.attempt_count + 1;
                        let backoff_secs = (30 * (1 << attempts.min(6))).min(3600);
                        let next_retry = now + backoff_secs;
                        let _ = recon_store
                            .create_or_update_item(
                                item.item_type,
                                &item.reference_id,
                                item.payment_hash.as_ref(),
                                item.session_id.as_ref(),
                                &item.reason,
                                Some("Backend confirmed not yet settled"),
                                Some(next_retry),
                            )
                            .await;
                    }
                    Err(e) => {
                        let attempts = item.attempt_count + 1;
                        let backoff_secs = (30 * (1 << attempts.min(6))).min(3600);
                        let next_retry = now + backoff_secs;
                        let _ = recon_store
                            .create_or_update_item(
                                item.item_type,
                                &item.reference_id,
                                item.payment_hash.as_ref(),
                                item.session_id.as_ref(),
                                &item.reason,
                                Some(&format!("Backend lookup error: {}", e)),
                                Some(next_retry),
                            )
                            .await;
                    }
                }
            }
        }

        let mut stats = self.stats.write().await;
        stats.last_scan_timestamp = Some(now);
        stats.scans_completed += 1;

        Ok(())
    }

    async fn handle_settled_invoice(
        &self,
        inv: &crate::node::storage::InvoiceRecord,
        invoice_store: &crate::node::storage::InvoiceStore,
        session_store: &SessionStore,
        recon_store: &ReconciliationStore,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let settle_res = invoice_store.mark_settled(&inv.payment_hash, now).await?;

        if settle_res == crate::node::storage::InvoiceSettlementResult::Settled
            || settle_res == crate::node::storage::InvoiceSettlementResult::AlreadySettled
        {
            let mut stats = self.stats.write().await;
            stats.invoices_settled += 1;
        }

        // If invoice is associated with a session:
        if let Some(ref session_id) = inv.session_id {
            if let Some(session) = session_store.get_session(session_id).await? {
                match session.status {
                    crate::node::storage::SessionStatus::PendingPayment => {
                        // Activate the session atomically
                        let _ = session_store
                            .activate_session(session_id, &inv.payment_hash)
                            .await;
                    }
                    crate::node::storage::SessionStatus::Closed => {
                        // Session is CLOSED! Late funding settlement detected.
                        // CRITICAL: NEVER reactivate a closed session.
                        tracing::warn!(
                            session_id = %session_id.0,
                            payment_hash = %inv.payment_hash.0,
                            "Settlement received for a closed session; recording reconciliation attention item"
                        );
                        let _ = recon_store
                            .create_or_update_item(
                                ReconciliationItemType::SessionLateFunding,
                                &inv.payment_hash.0,
                                Some(&inv.payment_hash),
                                Some(session_id),
                                "late_payment_on_closed_session",
                                Some("Invoice settled after session was already closed"),
                                None,
                            )
                            .await;
                    }
                    _ => {
                        // Already active/exhausted/expired
                    }
                }
            }
        }

        Ok(())
    }

    /// Spawn the reconciliation worker background loop.
    pub fn spawn(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            tracing::info!("Starting PayGuard Reconciliation Worker background task");

            // Initial startup scan
            if let Err(e) = self.run_reconciliation_pass().await {
                tracing::warn!("Reconciliation worker initial pass error: {}", e);
            }

            loop {
                tokio::select! {
                    _ = tokio::time::sleep(self.config.poll_interval) => {
                        if let Err(e) = self.run_reconciliation_pass().await {
                            tracing::warn!("Reconciliation worker periodic pass error: {}", e);
                        }
                    }
                    _ = shutdown.changed() => {
                        if *shutdown.borrow() {
                            tracing::info!("Reconciliation worker received shutdown signal; stopping gracefully");
                            break;
                        }
                    }
                }
            }
        })
    }
}
