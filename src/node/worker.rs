use crate::common::error::Result;
use crate::node::lightning::{LightningBackend, SettlementOutcome};
use crate::node::storage::reconciliation::{ReconciliationItemType, ReconciliationStore};
use crate::node::storage::session::SessionStore;
use crate::node::storage::Database;
use std::collections::HashMap;
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
        let mut checked_outcomes = HashMap::<String, SettlementOutcome>::new();

        // 1. Fetch bounded batch of pending invoices
        let pending_invoices = invoice_store
            .get_pending_invoices(self.config.batch_size)
            .await?;

        for inv in pending_invoices {
            if recon_store
                .invoice_retry_is_scheduled_after(&inv.payment_hash, now)
                .await?
            {
                continue;
            }

            let mut stats = self.stats.write().await;
            stats.invoices_checked += 1;
            drop(stats);

            // Query authoritative, typed settlement outcome from the backend
            let outcome = match self.lightning.check_settlement(&inv.payment_hash).await {
                Ok(out) => out,
                Err(e) => SettlementOutcome::BackendUnavailable {
                    error: e.to_string(),
                },
            };
            checked_outcomes.insert(inv.payment_hash.0.clone(), outcome.clone());

            match outcome {
                SettlementOutcome::ConfirmedSettled {
                    preimage,
                    settled_at,
                    amount_sats,
                } => {
                    let _ = (preimage, amount_sats);
                    self.handle_settled_invoice_with_ts(
                        &inv,
                        settled_at,
                        &invoice_store,
                        &session_store,
                        &recon_store,
                    )
                    .await?;
                }
                SettlementOutcome::ConfirmedExpired { .. } => {
                    if invoice_store.mark_expired(&inv.payment_hash).await? {
                        let mut stats = self.stats.write().await;
                        stats.invoices_expired += 1;
                    }
                }
                SettlementOutcome::ConfirmedCancelled { .. } => {
                    let _ = invoice_store.mark_cancelled(&inv.payment_hash).await;
                }
                SettlementOutcome::ConfirmedPending {
                    expires_at: backend_expires_at,
                } => {
                    // Check if both local and backend agree it is expired
                    if now >= inv.expires_at {
                        let is_backend_expired =
                            backend_expires_at.map(|exp| now >= exp).unwrap_or(true);
                        if is_backend_expired
                            && invoice_store.mark_expired(&inv.payment_hash).await?
                        {
                            let mut stats = self.stats.write().await;
                            stats.invoices_expired += 1;
                        }
                    }
                    // Otherwise still pending/open, no action
                }
                SettlementOutcome::BackendUnavailable { error } => {
                    // CRITICAL: NEVER mark expired or cancelled on backend unavailability
                    tracing::warn!(
                        payment_hash = %inv.payment_hash.0,
                        "Settlement backend unavailable for invoice: {}",
                        error
                    );
                    let mut stats = self.stats.write().await;
                    stats.last_error = Some(error);
                }
                SettlementOutcome::Unknown { details } => {
                    tracing::debug!(
                        payment_hash = %inv.payment_hash.0,
                        "Invoice lookup returned unknown/not found: {}",
                        details
                    );
                    // An unknown lookup is not authoritative expiry evidence. Keep the
                    // invoice pending and durably retry reconciliation with backoff.
                    let item_id = recon_store
                        .create_or_update_item(
                            ReconciliationItemType::Invoice,
                            &inv.payment_hash.0,
                            Some(&inv.payment_hash),
                            inv.session_id.as_ref(),
                            "invoice_lookup_unknown",
                            Some(&details),
                            None,
                        )
                        .await?;
                    let item = recon_store.get_item(&item_id).await?.ok_or_else(|| {
                        crate::common::error::Error::Internal(format!(
                            "Reconciliation item '{}' disappeared after creation",
                            item_id
                        ))
                    })?;
                    let attempts = (item.attempt_count + 1).min(6);
                    let backoff_secs = (30 * (1_i64 << attempts)).min(3600);
                    recon_store
                        .record_retry_attempt(
                            &item_id,
                            &format!("Unknown invoice in backend: {}", details),
                            now + backoff_secs,
                        )
                        .await?;
                }
                SettlementOutcome::InvalidOrConflictingEvidence { details } => {
                    tracing::warn!(
                        payment_hash = %inv.payment_hash.0,
                        "Conflicting or invalid settlement evidence received: {}",
                        details
                    );
                    let _ = recon_store
                        .create_or_update_item(
                            ReconciliationItemType::Invoice,
                            &inv.payment_hash.0,
                            Some(&inv.payment_hash),
                            inv.session_id.as_ref(),
                            "conflicting_settlement_evidence",
                            Some(&details),
                            None,
                        )
                        .await;
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
                let lookup = match checked_outcomes.get(&hash.0) {
                    Some(outcome) => Ok(outcome.clone()),
                    None => self.lightning.check_settlement(hash).await,
                };
                match lookup {
                    Ok(SettlementOutcome::ConfirmedSettled { settled_at, .. }) => {
                        let _ = invoice_store.mark_settled(hash, settled_at).await;
                        let _ = recon_store.mark_resolved(&item.id).await;
                        let mut stats = self.stats.write().await;
                        stats.items_reconciled += 1;
                    }
                    Ok(SettlementOutcome::ConfirmedExpired { .. }) => {
                        let _ = invoice_store.mark_expired(hash).await;
                        let _ = recon_store.mark_resolved(&item.id).await;
                    }
                    Ok(SettlementOutcome::ConfirmedCancelled { .. }) => {
                        let _ = invoice_store.mark_cancelled(hash).await;
                        let _ = recon_store.mark_resolved(&item.id).await;
                    }
                    Ok(SettlementOutcome::ConfirmedPending { .. }) => {
                        // Reschedule with backoff
                        let attempts = item.attempt_count + 1;
                        let backoff_secs = (30 * (1 << attempts.min(6))).min(3600);
                        let next_retry = now + backoff_secs;
                        recon_store
                            .record_retry_attempt(
                                &item.id,
                                "Backend confirmed still pending",
                                next_retry,
                            )
                            .await?;
                    }
                    Ok(SettlementOutcome::BackendUnavailable { error }) => {
                        let attempts = item.attempt_count + 1;
                        let backoff_secs = (30 * (1 << attempts.min(6))).min(3600);
                        let next_retry = now + backoff_secs;
                        recon_store
                            .record_retry_attempt(
                                &item.id,
                                &format!("Backend unavailable: {}", error),
                                next_retry,
                            )
                            .await?;
                    }
                    Ok(SettlementOutcome::Unknown { details }) => {
                        let attempts = item.attempt_count + 1;
                        let backoff_secs = (30 * (1 << attempts.min(6))).min(3600);
                        let next_retry = now + backoff_secs;
                        recon_store
                            .record_retry_attempt(
                                &item.id,
                                &format!("Unknown invoice in backend: {}", details),
                                next_retry,
                            )
                            .await?;
                    }
                    Ok(SettlementOutcome::InvalidOrConflictingEvidence { details }) => {
                        let _ = recon_store
                            .create_or_update_item(
                                item.item_type,
                                &item.reference_id,
                                item.payment_hash.as_ref(),
                                item.session_id.as_ref(),
                                "conflicting_settlement_evidence",
                                Some(&details),
                                None,
                            )
                            .await;
                    }
                    Err(e) => {
                        let attempts = item.attempt_count + 1;
                        let backoff_secs = (30 * (1 << attempts.min(6))).min(3600);
                        let next_retry = now + backoff_secs;
                        recon_store
                            .record_retry_attempt(
                                &item.id,
                                &format!("Backend lookup error: {}", e),
                                next_retry,
                            )
                            .await?;
                    }
                }
            }
        }

        let mut stats = self.stats.write().await;
        stats.last_scan_timestamp = Some(now);
        stats.scans_completed += 1;

        Ok(())
    }

    async fn handle_settled_invoice_with_ts(
        &self,
        inv: &crate::node::storage::InvoiceRecord,
        settled_at: i64,
        invoice_store: &crate::node::storage::InvoiceStore,
        session_store: &SessionStore,
        recon_store: &ReconciliationStore,
    ) -> Result<()> {
        let settle_res = invoice_store
            .mark_settled(&inv.payment_hash, settled_at)
            .await?;

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

    #[allow(dead_code)]
    async fn handle_settled_invoice(
        &self,
        inv: &crate::node::storage::InvoiceRecord,
        invoice_store: &crate::node::storage::InvoiceStore,
        session_store: &SessionStore,
        recon_store: &ReconciliationStore,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        self.handle_settled_invoice_with_ts(inv, now, invoice_store, session_store, recon_store)
            .await
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
