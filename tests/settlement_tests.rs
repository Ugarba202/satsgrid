use satsgrid::common::types::{PaymentHash, Satoshis};
use satsgrid::node::lightning::mock::MockLightningBackend;
use satsgrid::node::lightning::{LightningBackend, SettlementOutcome};
use satsgrid::node::storage::invoice::{InvoicePurpose, InvoiceSettlementResult, InvoiceStatus};
use satsgrid::node::storage::Database;
use tempfile::TempDir;

#[tokio::test]
async fn test_mock_settlement_outcomes_consistency() {
    let backend = MockLightningBackend::new();
    let invoice = backend
        .create_invoice(Satoshis(150), "test-memo")
        .await
        .expect("Invoice creation should succeed");

    // Initially, outcome should be ConfirmedPending
    let outcome1 = backend
        .check_settlement(&invoice.payment_hash)
        .await
        .expect("check_settlement should succeed");
    assert_eq!(
        outcome1,
        SettlementOutcome::ConfirmedPending { expires_at: None }
    );

    // Simulate payment
    backend.simulate_payment(&invoice.payment_hash).await;

    // Now, outcome should be ConfirmedSettled
    let outcome2 = backend
        .check_settlement(&invoice.payment_hash)
        .await
        .expect("check_settlement should succeed");
    match outcome2 {
        SettlementOutcome::ConfirmedSettled {
            amount_sats,
            settled_at,
            ..
        } => {
            assert_eq!(amount_sats, Some(150));
            assert!(settled_at > 0);
        }
        other => panic!("Expected ConfirmedSettled, got {:?}", other),
    }

    // Lookup on nonexistent payment hash yields Unknown
    let unknown_hash =
        PaymentHash("1111111111111111111111111111111111111111111111111111111111111111".to_string());
    let outcome3 = backend
        .check_settlement(&unknown_hash)
        .await
        .expect("check_settlement should succeed");
    assert!(matches!(outcome3, SettlementOutcome::Unknown { .. }));
}

#[tokio::test]
async fn test_mock_expiration_simulation() {
    let backend = MockLightningBackend::new();
    let invoice = backend
        .create_invoice(Satoshis(50), "expiry-test")
        .await
        .expect("Invoice creation should succeed");

    backend.simulate_expiration(&invoice.payment_hash).await;

    let outcome = backend
        .check_settlement(&invoice.payment_hash)
        .await
        .expect("check_settlement should succeed");
    assert!(matches!(
        outcome,
        SettlementOutcome::ConfirmedExpired { .. }
    ));
}

#[tokio::test]
async fn test_durable_invoice_settlement_cannot_be_overwritten_by_expiry() {
    let temp_dir = TempDir::new().unwrap();
    let db = Database::init(temp_dir.path()).await.unwrap();
    let invoice_store = db.invoice_store();

    let hash =
        PaymentHash("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string());
    let now = chrono::Utc::now().timestamp();

    invoice_store
        .create_invoice(
            &hash,
            "lnbc1...",
            Satoshis(100),
            InvoicePurpose::PayPerRequest,
            None,
            now + 3600,
        )
        .await
        .unwrap();

    // Settle the invoice
    let settle_res = invoice_store.mark_settled(&hash, now).await.unwrap();
    assert_eq!(settle_res, InvoiceSettlementResult::Settled);

    // Attempting to mark expired must be rejected / have no effect
    let expired_effect = invoice_store.mark_expired(&hash).await.unwrap();
    assert!(!expired_effect);

    // Attempting to mark cancelled must be rejected / have no effect
    let cancelled_effect = invoice_store.mark_cancelled(&hash).await.unwrap();
    assert!(!cancelled_effect);

    // Invoice status remains Settled
    let record = invoice_store.get_invoice(&hash).await.unwrap().unwrap();
    assert_eq!(record.status, InvoiceStatus::Settled);
}

#[tokio::test]
async fn test_late_settlement_after_local_expiry_conflict_detection() {
    let temp_dir = TempDir::new().unwrap();
    let db = Database::init(temp_dir.path()).await.unwrap();
    let invoice_store = db.invoice_store();

    let hash =
        PaymentHash("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string());
    let past = chrono::Utc::now().timestamp() - 100;

    // Invoice created in the past (already expired)
    invoice_store
        .create_invoice(
            &hash,
            "lnbc1...",
            Satoshis(200),
            InvoicePurpose::PayPerRequest,
            None,
            past,
        )
        .await
        .unwrap();

    // Marked expired locally
    let expired_ok = invoice_store.mark_expired(&hash).await.unwrap();
    assert!(expired_ok);

    // Now late settlement arrives! mark_settled returns ConflictExpired
    let settle_res = invoice_store.mark_settled(&hash, past + 50).await.unwrap();
    assert_eq!(settle_res, InvoiceSettlementResult::ConflictExpired);
}
