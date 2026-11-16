use base64::Engine;
use reqwest::Client;
use satsgrid::common::types::{PaymentHash, Satoshis};
use satsgrid::node::lightning::backend::{LightningBackend, SettlementOutcome};
use satsgrid::node::lightning::lnd::LndBackend;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_regtest_dual_node_lightning_payment_flow() {
    // 1. Spin up WireMock simulating Bob (Provider LND on REST port 8082)
    let bob_mock = MockServer::start().await;

    // 2. Spin up WireMock simulating Alice (Payer LND on REST port 8081)
    let alice_mock = MockServer::start().await;

    let hash_bytes = [42u8; 32];
    let hash_b64 = base64::engine::general_purpose::STANDARD.encode(hash_bytes);
    let hash_hex = hex::encode(hash_bytes);

    let preimage_bytes = [84u8; 32];
    let preimage_b64 = base64::engine::general_purpose::STANDARD.encode(preimage_bytes);
    let preimage_hex = hex::encode(preimage_bytes);

    // Real regtest BOLT11 invoice prefix: lnbcrt
    let regtest_bolt11 = format!("lnbcrt100n1pvjql8zpp5{}", hash_hex);

    // Mock Bob: Invoice Creation
    Mock::given(method("POST"))
        .and(path("/v1/invoices"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "r_hash": hash_b64,
            "payment_request": regtest_bolt11,
            "add_index": "1"
        })))
        .mount(&bob_mock)
        .await;

    // Mock Bob: Settlement Lookup
    Mock::given(method("GET"))
        .and(path(format!("/v1/invoice/{}", hash_hex)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "settled": true,
            "state": "SETTLED",
            "r_preimage": preimage_b64,
            "settle_date": "1731250000",
            "value": "100"
        })))
        .mount(&bob_mock)
        .await;

    // Mock Alice: Invoice Payment across channel
    Mock::given(method("POST"))
        .and(path("/v1/channels/transactions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "payment_preimage": preimage_b64,
            "payment_error": ""
        })))
        .mount(&alice_mock)
        .await;

    // Instantiate Provider Backend (Bob)
    let bob_backend = LndBackend::new(bob_mock.uri(), "bob_macaroon", Client::new());

    // Instantiate Payer Backend (Alice)
    let alice_backend = LndBackend::new(alice_mock.uri(), "alice_macaroon", Client::new());

    // Step A: Provider Bob creates a Regtest invoice
    let invoice = bob_backend
        .create_invoice(Satoshis(100), "Regtest AI Inference")
        .await
        .expect("Bob should create invoice");

    assert!(invoice.bolt11.starts_with("lnbcrt"));
    assert_eq!(invoice.payment_hash.0, hash_hex);

    // Step B: Payer Alice pays Bob's invoice across the channel
    let payment_result = alice_backend
        .pay_invoice(&invoice.bolt11)
        .await
        .expect("Alice should successfully route payment to Bob");

    assert_eq!(payment_result, preimage_hex);

    // Step C: Bob queries check_settlement to verify authoritative payment
    let settlement = bob_backend
        .check_settlement(&invoice.payment_hash)
        .await
        .expect("Bob should query settlement outcome");

    match settlement {
        SettlementOutcome::ConfirmedSettled {
            preimage,
            settled_at,
            amount_sats,
        } => {
            assert_eq!(preimage, Some(preimage_hex));
            assert_eq!(settled_at, 1731250000);
            assert_eq!(amount_sats, Some(100));
        }
        other => panic!("Expected ConfirmedSettled, got {:?}", other),
    }
}

#[tokio::test]
async fn test_regtest_transient_backend_outage_handling() {
    let bob_mock = MockServer::start().await;

    let hash_bytes = [55u8; 32];
    let hash_hex = hex::encode(hash_bytes);

    // Bob responds with 503 Service Unavailable
    Mock::given(method("GET"))
        .and(path(format!("/v1/invoice/{}", hash_hex)))
        .respond_with(ResponseTemplate::new(503).set_body_string("LND starting up"))
        .mount(&bob_mock)
        .await;

    let bob_backend = LndBackend::new(bob_mock.uri(), "bob_macaroon", Client::new());

    let outcome = bob_backend
        .check_settlement(&PaymentHash(hash_hex))
        .await
        .expect("Lookup should return typed outcome, not Err");

    match outcome {
        SettlementOutcome::BackendUnavailable { error } => {
            assert!(error.contains("5xx") || error.contains("503"));
        }
        other => panic!("Expected BackendUnavailable, got {:?}", other),
    }
}
