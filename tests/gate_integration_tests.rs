use satsgrid::common::types::{Satoshis, SessionId};
use satsgrid::node::gate::budget::{SessionBudgetManager, HEADER_REMAINING_BUDGET};
use satsgrid::node::gate::challenge::L402Challenge;
use satsgrid::node::gate::macaroon::{Caveat, MacaroonService};
use satsgrid::node::gate::verify::L402Verifier;
use satsgrid::node::lightning::backend::LightningBackend;
use satsgrid::node::lightning::mock::MockLightningBackend;
use uuid::Uuid;

#[tokio::test]
async fn test_end_to_end_l402_handshake_and_budget_lifecycle() {
    // 1. Setup Node components: Macaroon service and Lightning backend
    let root_key = b"super-secret-node-root-key-32-b".to_vec();
    let macaroon_service = MacaroonService::new(root_key, "satsgrid-node-alpha");
    let lightning_backend = MockLightningBackend::new();
    let budget_manager = SessionBudgetManager::new();

    // 2. Caller initiates a multi-step session with a 500 satoshis budget
    let session_uuid = Uuid::new_v4();
    let session_id = SessionId(session_uuid);
    let total_budget = Satoshis(500);

    // Node opens the session in the budget manager
    budget_manager.open_session(&session_id, total_budget).await;

    // 3. Node generates an invoice for the session (or pay-per-request)
    let session_invoice = lightning_backend
        .create_invoice(total_budget, "Infernos Session Budget 500 sats")
        .await
        .expect("Invoice generation should succeed");

    // 4. Node mints an L402 Macaroon bound to the invoice payment hash and caveats
    let now_ts = 1750000000;
    let expiry_ts = now_ts + 3600; // 1 hour validity
    let caveats = vec![
        Caveat::ExpiresAt(expiry_ts),
        Caveat::Model("llama3".to_string()),
        Caveat::Session(session_uuid.to_string()),
        Caveat::Budget(total_budget.0),
    ];

    let macaroon = macaroon_service
        .mint(&session_invoice.payment_hash, caveats)
        .expect("Macaroon minting should succeed");

    // 5. Node returns HTTP 402 with WWW-Authenticate header
    let challenge = L402Challenge::from_components(&macaroon, &session_invoice)
        .expect("Challenge creation should succeed");
    let challenge_header = challenge.to_header_value();
    assert!(challenge_header.starts_with("L402 token=\""));
    assert!(challenge_header.contains("invoice=\"lnbc"));

    // 6. Caller receives the 402 challenge header, parses it, and inspects details
    let parsed_challenge = L402Challenge::from_header_value(&challenge_header)
        .expect("Caller should parse challenge header");
    assert_eq!(parsed_challenge.macaroon, challenge.macaroon);
    assert_eq!(parsed_challenge.invoice, session_invoice.bolt11);

    // 7. Caller pays the Lightning invoice and obtains the cryptographic preimage
    // In this integration test with MockLightningBackend, simulate payment:
    lightning_backend
        .simulate_payment(&session_invoice.payment_hash)
        .await;

    // Caller constructs the preimage (which hashes to the invoice payment hash)
    // Note: for MockLightningBackend, we verify that preimage hashing matches
    let preimage_bytes = [0x5au8; 32];
    let preimage_hex = hex::encode(preimage_bytes);
    let known_payment_hash =
        L402Verifier::hash_preimage(&preimage_hex).expect("Preimage hashing should succeed");

    // Re-mint a macaroon bound to this exact known preimage hash for strict cryptographic validation
    lightning_backend
        .simulate_payment(&known_payment_hash)
        .await;
    let verified_macaroon = macaroon_service
        .mint(
            &known_payment_hash,
            vec![
                Caveat::ExpiresAt(expiry_ts),
                Caveat::Model("llama3".to_string()),
                Caveat::Session(session_uuid.to_string()),
                Caveat::Budget(total_budget.0),
            ],
        )
        .unwrap();

    let client_token = verified_macaroon.to_base64().unwrap();
    let auth_header = format!("L402 {}:{}", client_token, preimage_hex);

    // 8. Gate verifies the incoming Authorization header
    let verified_creds = L402Verifier::verify_header(
        &macaroon_service,
        &lightning_backend,
        &auth_header,
        Some("llama3"),
        now_ts,
    )
    .await
    .expect("L402 verification should succeed");

    assert_eq!(verified_creds.macaroon.identifier, known_payment_hash.0);

    // 9. Extract session caveats and perform atomic budget debit for Request 1 (cost: 50 sats)
    let (extracted_session, _) =
        SessionBudgetManager::extract_session_caveats(&verified_creds.macaroon);
    assert_eq!(extracted_session, Some(SessionId(session_uuid)));

    let remaining_after_req1 = budget_manager
        .debit_session(&extracted_session.unwrap(), Satoshis(50))
        .await
        .expect("Request 1 debit should succeed");
    assert_eq!(remaining_after_req1, Satoshis(450));

    // Gate injects X-SatsGrid-Remaining-Budget-Sats header
    let remaining_header_val = remaining_after_req1.0.to_string();
    assert_eq!(remaining_header_val, "450");
    assert_eq!(HEADER_REMAINING_BUDGET, "x-satsgrid-remaining-budget-sats");

    // 10. Request 2 (cost: 400 sats)
    let remaining_after_req2 = budget_manager
        .debit_session(&session_id, Satoshis(400))
        .await
        .expect("Request 2 debit should succeed");
    assert_eq!(remaining_after_req2, Satoshis(50));

    // 11. Request 3 (cost: 60 sats) -> exceeds remaining 50 sats, must be refused!
    let req3_result = budget_manager
        .debit_session(&session_id, Satoshis(60))
        .await;
    assert!(req3_result.is_err());
    let err_msg = req3_result.unwrap_err().to_string();
    assert!(err_msg.contains("exceeds remaining budget"));

    // Ensure remaining budget was not decremented on failed debit
    let final_balance = budget_manager
        .get_session(&session_id)
        .await
        .unwrap()
        .remaining();
    assert_eq!(final_balance, Satoshis(50));
}
