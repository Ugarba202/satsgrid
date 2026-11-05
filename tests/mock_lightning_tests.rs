use satsgrid::common::types::Satoshis;
use satsgrid::node::lightning::backend::LightningBackend;
use satsgrid::node::lightning::mock::MockLightningBackend;

#[tokio::test]
async fn test_mock_backend_create_and_settle() {
    let backend = MockLightningBackend::new();

    // 1. Create an invoice
    let amount = Satoshis(100);
    let invoice = backend
        .create_invoice(amount, "Test invoice")
        .await
        .expect("Failed to create invoice");

    assert_eq!(invoice.amount, amount);
    assert!(invoice.bolt11.starts_with("lnbc100mock"));
    assert_eq!(invoice.payment_hash.0.len(), 64);

    // 2. Check settlement before payment (should be false)
    let is_settled = backend
        .is_invoice_settled(&invoice.payment_hash)
        .await
        .expect("Failed to check settlement");
    assert!(!is_settled);

    // 3. Simulate payment in the mock and check preimage
    let preimage = backend
        .pay_invoice(&invoice.bolt11)
        .await
        .expect("Failed to pay mock invoice");
    assert_eq!(preimage.len(), 64);

    // 4. Verify SHA256(preimage) == payment_hash
    use sha2::{Digest, Sha256};
    let preimage_bytes = hex::decode(&preimage).unwrap();
    let mut hasher = Sha256::new();
    hasher.update(preimage_bytes);
    let calculated_hash = hex::encode(hasher.finalize());
    assert_eq!(calculated_hash, invoice.payment_hash.0);

    // 5. Check settlement after payment (should be true)
    let is_settled_after = backend
        .is_invoice_settled(&invoice.payment_hash)
        .await
        .expect("Failed to check settlement");
    assert!(is_settled_after);
}
