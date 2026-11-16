use satsgrid::node::receipt::{ReceiptPayload, ReceiptSigner, ED25519_ALGORITHM_NAME};
use tempfile::tempdir;

#[test]
fn test_receipt_signing_and_verification_success() {
    let dir = tempdir().expect("tempdir");
    let signer =
        ReceiptSigner::load_or_generate(dir.path(), "provider_pubkey_123").expect("signer");

    let payload = ReceiptPayload {
        receipt_id: "rcpt_001".to_string(),
        schema_version: 1,
        key_id: signer.key_id().to_string(),
        provider_id: "provider_pubkey_123".to_string(),
        request_id: "req_001".to_string(),
        session_id: Some("session_abc".to_string()),
        model: "llama-3.2-3b".to_string(),
        inference_status: "completed".to_string(),
        payment_status: "settled".to_string(),
        payment_hash: Some("deadbeef".to_string()),
        approved_max_sats: 100,
        actual_charge_sats: 45,
        currency: "sats".to_string(),
        prompt_tokens: Some(10),
        completion_tokens: Some(25),
        error_code: None,
        reconciliation_id: None,
        supersedes_receipt_id: None,
        created_at: 1762768000,
    };

    let signed = signer.sign_receipt(payload.clone()).expect("sign");
    assert_eq!(signed.algorithm, ED25519_ALGORITHM_NAME);
    assert_eq!(&signed.key_id, signer.key_id());

    // 1. Verify with explicit expected pubkey
    let verify_res = ReceiptSigner::verify_signature(&signed, Some(signer.public_key_hex()));
    assert!(
        verify_res.is_ok(),
        "Signature verification failed: {:?}",
        verify_res
    );

    // 2. Verify with pubkey embedded in receipt
    let verify_embed = ReceiptSigner::verify_signature(&signed, None);
    assert!(verify_embed.is_ok());
}

#[test]
fn test_receipt_verification_rejects_tampered_payload() {
    let dir = tempdir().expect("tempdir");
    let signer = ReceiptSigner::load_or_generate(dir.path(), "provider_1").expect("signer");

    let payload = ReceiptPayload {
        receipt_id: "rcpt_002".to_string(),
        schema_version: 1,
        key_id: signer.key_id().to_string(),
        provider_id: "provider_1".to_string(),
        request_id: "req_002".to_string(),
        session_id: None,
        model: "llama-3.2-3b".to_string(),
        inference_status: "completed".to_string(),
        payment_status: "settled".to_string(),
        payment_hash: None,
        approved_max_sats: 100,
        actual_charge_sats: 50,
        currency: "sats".to_string(),
        prompt_tokens: Some(10),
        completion_tokens: Some(20),
        error_code: None,
        reconciliation_id: None,
        supersedes_receipt_id: None,
        created_at: 1762768000,
    };

    let mut signed = signer.sign_receipt(payload).expect("sign");

    // Tamper with actual_charge_sats (reduce from 50 to 5)
    signed.payload.actual_charge_sats = 5;

    let res = ReceiptSigner::verify_signature(&signed, Some(signer.public_key_hex()));
    assert!(res.is_err(), "Verification should fail on tampered payload");
}

#[test]
fn test_receipt_key_persistence_and_stability_across_restarts() {
    let dir = tempdir().expect("tempdir");

    // First load creates key
    let signer1 = ReceiptSigner::load_or_generate(dir.path(), "provider_node_1").expect("load 1");
    let pubkey1 = signer1.public_key_hex().to_string();
    let key_id1 = signer1.key_id().to_string();

    // Second load reads existing key
    let signer2 = ReceiptSigner::load_or_generate(dir.path(), "provider_node_1").expect("load 2");
    assert_eq!(signer2.public_key_hex(), pubkey1);
    assert_eq!(signer2.key_id(), key_id1);

    // Sign with signer1, verify with signer2
    let payload = ReceiptPayload {
        receipt_id: "rcpt_003".to_string(),
        schema_version: 1,
        key_id: key_id1,
        provider_id: "provider_node_1".to_string(),
        request_id: "req_003".to_string(),
        session_id: None,
        model: "llama-3.2-3b".to_string(),
        inference_status: "completed".to_string(),
        payment_status: "settled".to_string(),
        payment_hash: None,
        approved_max_sats: 10,
        actual_charge_sats: 10,
        currency: "sats".to_string(),
        prompt_tokens: None,
        completion_tokens: None,
        error_code: None,
        reconciliation_id: None,
        supersedes_receipt_id: None,
        created_at: 1762768000,
    };

    let signed = signer1.sign_receipt(payload).expect("sign");
    assert!(ReceiptSigner::verify_signature(&signed, Some(signer2.public_key_hex())).is_ok());
}
