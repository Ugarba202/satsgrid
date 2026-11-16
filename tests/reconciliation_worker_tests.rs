use satsgrid::common::types::{PaymentHash, Satoshis, SessionId};
use satsgrid::node::lightning::invoice::Invoice;
use satsgrid::node::lightning::mock::MockLightningBackend;
use satsgrid::node::lightning::{LightningBackend, SettlementOutcome};
use satsgrid::node::storage::{
    Database, InvoicePurpose, InvoiceStatus, ReconciliationItemType, RequestMode,
};
use satsgrid::node::worker::{ReconciliationWorker, WorkerConfig};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;

struct CountingBackend {
    outcome: SettlementOutcome,
    lookups: AtomicUsize,
}

#[async_trait::async_trait]
impl LightningBackend for CountingBackend {
    async fn create_invoice(
        &self,
        _amount_sats: Satoshis,
        _memo: &str,
    ) -> satsgrid::common::error::Result<Invoice> {
        Err(satsgrid::common::error::Error::Lightning(
            "invoice creation is not part of this test".to_string(),
        ))
    }

    async fn is_invoice_settled(
        &self,
        _payment_hash: &PaymentHash,
    ) -> satsgrid::common::error::Result<bool> {
        Ok(false)
    }

    async fn check_settlement(
        &self,
        _payment_hash: &PaymentHash,
    ) -> satsgrid::common::error::Result<SettlementOutcome> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        Ok(self.outcome.clone())
    }
}

#[tokio::test]
async fn test_startup_recovery_with_pending_and_settled_invoices() {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));
    let mock_lightning = Arc::new(MockLightningBackend::new());

    let invoice_store = db.invoice_store();
    let payment_hash =
        PaymentHash("aabbcc11223344556677889900aabbcc11223344556677889900aabbcc1122".to_string());
    let bolt11 = format!("lnbc100mock{}", payment_hash.0);

    // 1. Create invoice in pending state
    invoice_store
        .create_invoice(
            &payment_hash,
            &bolt11,
            Satoshis(100),
            InvoicePurpose::PayPerRequest,
            None,
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .expect("create invoice");

    assert_eq!(
        invoice_store
            .get_invoice(&payment_hash)
            .await
            .unwrap()
            .unwrap()
            .status,
        InvoiceStatus::Pending
    );

    // 2. Simulate payment in mock lightning
    mock_lightning.simulate_payment(&payment_hash).await;

    // 3. Run reconciliation pass
    let worker = ReconciliationWorker::new(
        db.clone(),
        mock_lightning.clone(),
        WorkerConfig {
            poll_interval: Duration::from_millis(100),
            batch_size: 10,
        },
    );

    worker.run_reconciliation_pass().await.expect("reconcile");

    // 4. Verify invoice status transitioned to settled
    let inv = invoice_store
        .get_invoice(&payment_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inv.status, InvoiceStatus::Settled);
    assert!(inv.settled_at.is_some());

    let stats = worker.stats().await;
    assert_eq!(stats.invoices_settled, 1);
}

#[tokio::test]
async fn test_unknown_invoice_remains_pending_and_retries_with_backoff() {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));
    let backend = Arc::new(CountingBackend {
        outcome: SettlementOutcome::Unknown {
            details: "unknown invoice".to_string(),
        },
        lookups: AtomicUsize::new(0),
    });

    let invoice_store = db.invoice_store();
    let payment_hash =
        PaymentHash("11223344556677889900aabbcc11223344556677889900aabbcc1122334455".to_string());
    let bolt11 = format!("lnbc50mock{}", payment_hash.0);

    // The local invoice is expired, but the backend has no record of this payment hash.
    let past = chrono::Utc::now().timestamp() - 60;
    invoice_store
        .create_invoice(
            &payment_hash,
            &bolt11,
            Satoshis(50),
            InvoicePurpose::PayPerRequest,
            None,
            past,
        )
        .await
        .expect("create invoice");

    let worker = ReconciliationWorker::new(db.clone(), backend.clone(), WorkerConfig::default());

    worker.run_reconciliation_pass().await.expect("reconcile");

    let inv = invoice_store
        .get_invoice(&payment_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inv.status, InvoiceStatus::Pending);

    let items = db
        .reconciliation_store()
        .list_items(None, 10)
        .await
        .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].item_type, ReconciliationItemType::Invoice);
    assert_eq!(items[0].status.as_str(), "unresolved");
    assert_eq!(items[0].attempt_count, 1);
    assert!(items[0].last_attempt_at.is_some());
    let retry_delay = items[0].next_retry_at.unwrap() - items[0].last_attempt_at.unwrap();
    assert!((59..=60).contains(&retry_delay));
    assert!(items[0]
        .error_details
        .as_deref()
        .unwrap_or_default()
        .contains("Unknown invoice in backend"));

    let stats = worker.stats().await;
    assert_eq!(stats.invoices_expired, 0);
    assert_eq!(backend.lookups.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn future_invoice_retry_skips_pending_scan_without_changing_retry_metadata() {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));
    let backend = Arc::new(CountingBackend {
        outcome: SettlementOutcome::Unknown {
            details: "unknown invoice".to_string(),
        },
        lookups: AtomicUsize::new(0),
    });
    let payment_hash =
        PaymentHash("223344556677889900aabbcc11223344556677889900aabbcc11223344556677".to_string());
    db.invoice_store()
        .create_invoice(
            &payment_hash,
            "test-only-invoice-placeholder",
            Satoshis(50),
            InvoicePurpose::PayPerRequest,
            None,
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .expect("persist local pending invoice");
    let item_id = db
        .reconciliation_store()
        .create_or_update_item(
            ReconciliationItemType::Invoice,
            &payment_hash.0,
            Some(&payment_hash),
            None,
            "invoice_lookup_unknown",
            Some("previous unknown lookup"),
            None,
        )
        .await
        .expect("create unresolved invoice item");
    let last_attempt_at = 1_700_000_000;
    let next_retry_at = chrono::Utc::now().timestamp() + 600;
    sqlx::query("UPDATE reconciliation_items SET attempt_count = 3, last_attempt_at = ?1, next_retry_at = ?2 WHERE id = ?3")
        .bind(last_attempt_at)
        .bind(next_retry_at)
        .bind(&item_id)
        .execute(db.pool())
        .await
        .expect("schedule future retry");

    let worker = ReconciliationWorker::new(db.clone(), backend.clone(), WorkerConfig::default());
    worker
        .run_reconciliation_pass()
        .await
        .expect("skip invoice until its retry is due");

    assert_eq!(backend.lookups.load(Ordering::SeqCst), 0);
    assert_eq!(worker.stats().await.invoices_checked, 0);
    let item = db
        .reconciliation_store()
        .get_item(&item_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.status.as_str(), "unresolved");
    assert_eq!(item.attempt_count, 3);
    assert_eq!(item.last_attempt_at, Some(last_attempt_at));
    assert_eq!(item.next_retry_at, Some(next_retry_at));
}

#[tokio::test]
async fn due_invoice_retry_is_checked_and_rescheduled_once() {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));
    let backend = Arc::new(CountingBackend {
        outcome: SettlementOutcome::Unknown {
            details: "unknown invoice".to_string(),
        },
        lookups: AtomicUsize::new(0),
    });
    let payment_hash =
        PaymentHash("3344556677889900aabbcc11223344556677889900aabbcc1122334455667788".to_string());
    db.invoice_store()
        .create_invoice(
            &payment_hash,
            "test-only-invoice-placeholder",
            Satoshis(50),
            InvoicePurpose::PayPerRequest,
            None,
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .expect("persist local pending invoice");
    let item_id = db
        .reconciliation_store()
        .create_or_update_item(
            ReconciliationItemType::Invoice,
            &payment_hash.0,
            Some(&payment_hash),
            None,
            "invoice_lookup_unknown",
            Some("previous unknown lookup"),
            None,
        )
        .await
        .expect("create unresolved invoice item");
    let old_last_attempt_at = 1_700_000_000;
    sqlx::query("UPDATE reconciliation_items SET attempt_count = 2, last_attempt_at = ?1, next_retry_at = ?2 WHERE id = ?3")
        .bind(old_last_attempt_at)
        .bind(chrono::Utc::now().timestamp() - 1)
        .bind(&item_id)
        .execute(db.pool())
        .await
        .expect("make retry due");

    let worker = ReconciliationWorker::new(db.clone(), backend.clone(), WorkerConfig::default());
    worker
        .run_reconciliation_pass()
        .await
        .expect("perform due retry");

    assert_eq!(backend.lookups.load(Ordering::SeqCst), 1);
    let item = db
        .reconciliation_store()
        .get_item(&item_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.status.as_str(), "unresolved");
    assert_eq!(item.attempt_count, 3);
    assert!(item.last_attempt_at.unwrap() >= old_last_attempt_at);
    let retry_delay = item.next_retry_at.unwrap() - item.last_attempt_at.unwrap();
    assert!((239..=240).contains(&retry_delay));
}

#[tokio::test]
async fn eligible_reconciliation_item_without_pending_invoice_is_checked() {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));
    let backend = Arc::new(CountingBackend {
        outcome: SettlementOutcome::Unknown {
            details: "unknown invoice".to_string(),
        },
        lookups: AtomicUsize::new(0),
    });
    let payment_hash =
        PaymentHash("44556677889900aabbcc11223344556677889900aabbcc112233445566778899".to_string());
    let item_id = db
        .reconciliation_store()
        .create_or_update_item(
            ReconciliationItemType::Invoice,
            &payment_hash.0,
            Some(&payment_hash),
            None,
            "invoice_lookup_unknown",
            Some("previous unknown lookup"),
            None,
        )
        .await
        .expect("create invoice reconciliation item");
    let worker = ReconciliationWorker::new(db.clone(), backend.clone(), WorkerConfig::default());

    worker
        .run_reconciliation_pass()
        .await
        .expect("process eligible item without pending invoice");

    assert_eq!(backend.lookups.load(Ordering::SeqCst), 1);
    let item = db
        .reconciliation_store()
        .get_item(&item_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.status.as_str(), "unresolved");
    assert_eq!(item.attempt_count, 1);
    assert!(item.next_retry_at.is_some());
}

#[tokio::test]
async fn backend_unavailable_due_invoice_is_checked_only_once_per_pass() {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));
    let backend = Arc::new(CountingBackend {
        outcome: SettlementOutcome::BackendUnavailable {
            error: "temporary outage".to_string(),
        },
        lookups: AtomicUsize::new(0),
    });
    let payment_hash =
        PaymentHash("556677889900aabbcc11223344556677889900aabbcc11223344556677889900".to_string());
    db.invoice_store()
        .create_invoice(
            &payment_hash,
            "test-only-invoice-placeholder",
            Satoshis(50),
            InvoicePurpose::PayPerRequest,
            None,
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .expect("persist local pending invoice");
    let item_id = db
        .reconciliation_store()
        .create_or_update_item(
            ReconciliationItemType::Invoice,
            &payment_hash.0,
            Some(&payment_hash),
            None,
            "invoice_lookup_unknown",
            Some("previous lookup failed"),
            None,
        )
        .await
        .expect("create due invoice reconciliation item");
    sqlx::query(
        "UPDATE reconciliation_items SET attempt_count = 2, next_retry_at = ?1 WHERE id = ?2",
    )
    .bind(chrono::Utc::now().timestamp() - 1)
    .bind(&item_id)
    .execute(db.pool())
    .await
    .expect("make retry due");

    let worker = ReconciliationWorker::new(db.clone(), backend.clone(), WorkerConfig::default());
    worker
        .run_reconciliation_pass()
        .await
        .expect("process backend-unavailable outcome once");

    assert_eq!(backend.lookups.load(Ordering::SeqCst), 1);
    let item = db
        .reconciliation_store()
        .get_item(&item_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.status.as_str(), "unresolved");
    assert_eq!(item.attempt_count, 3);
    assert!(item.last_attempt_at.is_some());
    assert!(item.next_retry_at.unwrap() > chrono::Utc::now().timestamp());
    assert!(worker
        .stats()
        .await
        .last_error
        .as_deref()
        .unwrap_or_default()
        .contains("temporary outage"));
}

#[tokio::test]
async fn confirmed_expired_outcome_marks_invoice_expired() {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));
    let backend = Arc::new(CountingBackend {
        outcome: SettlementOutcome::ConfirmedExpired {
            expired_at: chrono::Utc::now().timestamp(),
        },
        lookups: AtomicUsize::new(0),
    });
    let payment_hash =
        PaymentHash("aabbcc11223344556677889900aabbcc11223344556677889900aabbcc1122".to_string());

    db.invoice_store()
        .create_invoice(
            &payment_hash,
            "test-only-invoice-placeholder",
            Satoshis(50),
            InvoicePurpose::PayPerRequest,
            None,
            chrono::Utc::now().timestamp() - 60,
        )
        .await
        .expect("persist locally expired invoice");
    let item_id = db
        .reconciliation_store()
        .create_or_update_item(
            ReconciliationItemType::Invoice,
            &payment_hash.0,
            Some(&payment_hash),
            None,
            "invoice_lookup_unknown",
            Some("previous unknown lookup"),
            None,
        )
        .await
        .expect("create due reconciliation item");

    let worker = ReconciliationWorker::new(db.clone(), backend.clone(), WorkerConfig::default());
    worker
        .run_reconciliation_pass()
        .await
        .expect("reconcile confirmed expiration");

    let invoice = db
        .invoice_store()
        .get_invoice(&payment_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(invoice.status, InvoiceStatus::Expired);
    assert_eq!(worker.stats().await.invoices_expired, 1);
    assert_eq!(backend.lookups.load(Ordering::SeqCst), 1);
    let item = db
        .reconciliation_store()
        .get_item(&item_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.status.as_str(), "resolved");
}

#[tokio::test]
async fn test_closed_session_with_late_settled_funding_invoice() {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));
    let mock_lightning = Arc::new(MockLightningBackend::new());

    let session_store = db.session_store();
    let invoice_store = db.invoice_store();
    let recon_store = db.reconciliation_store();

    let session_id = SessionId(uuid::Uuid::new_v4());
    let payment_hash =
        PaymentHash("deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef".to_string());
    let bolt11 = format!("lnbc1000mock{}", payment_hash.0);

    // 1. Create active session and invoice
    session_store
        .create_session(
            &session_id,
            Some(&payment_hash),
            Satoshis(1000),
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .expect("create session");

    invoice_store
        .create_invoice(
            &payment_hash,
            &bolt11,
            Satoshis(1000),
            InvoicePurpose::SessionFunding,
            Some(&session_id),
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .expect("create invoice");

    // 2. Explicitly close session before the invoice settles
    let close_res = session_store
        .close_session(&session_id, Some("user_cancelled_before_funding"))
        .await
        .expect("close session");
    match close_res {
        satsgrid::node::storage::SessionCloseResult::Closed { .. } => {}
        _ => panic!("Expected Closed result"),
    }

    let session_before = session_store
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        session_before.status,
        satsgrid::node::storage::SessionStatus::Closed
    );

    // 3. Late Lightning settlement occurs
    mock_lightning.simulate_payment(&payment_hash).await;

    // 4. Run reconciliation pass
    let worker =
        ReconciliationWorker::new(db.clone(), mock_lightning.clone(), WorkerConfig::default());
    worker.run_reconciliation_pass().await.expect("reconcile");

    // 5. INVARIANTS:
    // - Invoice must be recorded as settled
    let inv = invoice_store
        .get_invoice(&payment_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inv.status, InvoiceStatus::Settled);

    // - Session must REMAIN closed (NO resurrection)
    let session_after = session_store
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        session_after.status,
        satsgrid::node::storage::SessionStatus::Closed
    );

    // - Reconciliation attention item must be recorded
    let unresolved_count = recon_store
        .count_unresolved()
        .await
        .expect("count unresolved");
    assert_eq!(unresolved_count, 1);

    let items = recon_store
        .get_eligible_retry_items(10)
        .await
        .expect("get items");
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].item_type,
        ReconciliationItemType::SessionLateFunding
    );
    assert_eq!(items[0].reason, "late_payment_on_closed_session");
}

#[tokio::test]
async fn test_in_flight_request_recovery_undispatched_vs_processing() {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));
    let req_store = db.request_store();

    let session_id = SessionId(uuid::Uuid::new_v4());
    let session_store = db.session_store();
    session_store
        .create_pending_session(
            &session_id,
            None,
            Satoshis(100),
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .expect("create session");

    // 1. Create a reserved request with no started_at (definitely undispatched)
    req_store
        .create_request(
            "req_undispatched",
            None,
            Some(&session_id),
            None,
            None,
            "mock-model",
            RequestMode::NonStreaming,
            Satoshis(10),
        )
        .await
        .expect("create req 1");

    // 2. Create another reserved request, mark it processing (dispatched upstream)
    req_store
        .create_request(
            "req_dispatched",
            None,
            Some(&session_id),
            None,
            None,
            "mock-model",
            RequestMode::NonStreaming,
            Satoshis(10),
        )
        .await
        .expect("create req 2");
    req_store
        .mark_processing("req_dispatched")
        .await
        .expect("mark processing");

    // 3. Simulate crash recovery on startup
    let undispatched = req_store
        .recover_in_flight_requests()
        .await
        .expect("recover");
    assert_eq!(undispatched.len(), 0); // No session reservations

    // 4. Verify request 1 transitioned to failed
    let req1 = req_store
        .get_request("req_undispatched")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(req1.status, satsgrid::node::storage::RequestStatus::Failed);
    assert_eq!(
        req1.error_code.as_deref(),
        Some("server_crash_prior_to_dispatch")
    );

    // 5. Verify request 2 transitioned to reconciliation_required
    let req2 = req_store
        .get_request("req_dispatched")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        req2.status,
        satsgrid::node::storage::RequestStatus::ReconciliationRequired
    );
    assert_eq!(
        req2.error_code.as_deref(),
        Some("unresolved_crash_during_execution")
    );

    let items = db
        .reconciliation_store()
        .list_items(None, 10)
        .await
        .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].item_type, ReconciliationItemType::InferenceRequest);
    assert_eq!(items[0].reference_id, "req_dispatched");
    assert_eq!(items[0].session_id, Some(session_id));
    assert_eq!(
        items[0].status,
        satsgrid::node::storage::ReconciliationStatus::Unresolved
    );
    assert_eq!(items[0].reason, "unresolved_crash_during_execution");
}

#[tokio::test]
async fn test_worker_graceful_shutdown() {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));
    let mock_lightning = Arc::new(MockLightningBackend::new());

    let worker = Arc::new(ReconciliationWorker::new(
        db,
        mock_lightning,
        WorkerConfig {
            poll_interval: Duration::from_millis(50),
            batch_size: 10,
        },
    ));

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let handle = worker.spawn(shutdown_rx);

    tokio::time::sleep(Duration::from_millis(120)).await;

    // Send shutdown signal
    shutdown_tx.send(true).expect("send shutdown");

    // Await handle termination within timeout
    let res = tokio::time::timeout(Duration::from_secs(2), handle).await;
    assert!(
        res.is_ok(),
        "Worker did not stop gracefully within 2 seconds"
    );
}

#[tokio::test]
async fn worker_retry_increments_attempt_and_schedules_backoff() {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));
    let payment_hash =
        PaymentHash("aabbcc11223344556677889900aabbcc11223344556677889900aabbcc1122".to_string());
    let item_id = db
        .reconciliation_store()
        .create_or_update_item(
            ReconciliationItemType::InferenceRequest,
            "retry-backoff-request",
            Some(&payment_hash),
            None,
            "ambiguous_dispatch",
            Some("upstream result unknown"),
            None,
        )
        .await
        .expect("create reconciliation item");
    let mock_lightning = Arc::new(MockLightningBackend::new());
    let worker = ReconciliationWorker::new(
        db.clone(),
        mock_lightning,
        WorkerConfig {
            poll_interval: Duration::from_millis(100),
            batch_size: 10,
        },
    );

    let before = chrono::Utc::now().timestamp();
    worker
        .run_reconciliation_pass()
        .await
        .expect("worker records lookup retry");
    let after = chrono::Utc::now().timestamp();

    let item = db
        .reconciliation_store()
        .get_item(&item_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.attempt_count, 1);
    assert!(item.last_attempt_at.is_some());
    assert!(item.last_attempt_at.unwrap() >= before);
    assert!(item.last_attempt_at.unwrap() <= after);
    // First retry uses 30 * 2^1 = 60 seconds of backoff.
    assert_eq!(item.next_retry_at, Some(item.last_attempt_at.unwrap() + 60));
}
