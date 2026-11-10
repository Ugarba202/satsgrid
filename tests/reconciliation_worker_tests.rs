use satsgrid::common::types::{PaymentHash, Satoshis, SessionId};
use satsgrid::node::lightning::mock::MockLightningBackend;
use satsgrid::node::storage::{
    Database, InvoicePurpose, InvoiceStatus, ReconciliationItemType, RequestMode,
};
use satsgrid::node::worker::{ReconciliationWorker, WorkerConfig};
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;

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
async fn test_startup_recovery_with_expired_invoices() {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));
    let mock_lightning = Arc::new(MockLightningBackend::new());

    let invoice_store = db.invoice_store();
    let payment_hash =
        PaymentHash("11223344556677889900aabbcc11223344556677889900aabbcc1122334455".to_string());
    let bolt11 = format!("lnbc50mock{}", payment_hash.0);

    // Create an already-expired invoice
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

    let worker =
        ReconciliationWorker::new(db.clone(), mock_lightning.clone(), WorkerConfig::default());

    worker.run_reconciliation_pass().await.expect("reconcile");

    let inv = invoice_store
        .get_invoice(&payment_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inv.status, InvoiceStatus::Expired);

    let stats = worker.stats().await;
    assert_eq!(stats.invoices_expired, 1);
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
