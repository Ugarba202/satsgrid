use base64::Engine;
use reqwest::Client;
use satsgrid::common::types::{PaymentHash, Satoshis};
use satsgrid::config::schema::{BitcoinNetwork, LightningBackendType, LightningConfig};
use satsgrid::node::lightning::backend::LightningBackend;
use satsgrid::node::lightning::lnd::LndBackend;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_lnd_backend_create_invoice_success() {
    let mock_lnd = MockServer::start().await;

    let hash_bytes = [7u8; 32];
    let hash_b64 = base64::engine::general_purpose::STANDARD.encode(hash_bytes);
    let hash_hex = hex::encode(hash_bytes);
    let bolt11_inv = "lnbctest100n1pvjql8zpp5...";

    Mock::given(method("POST"))
        .and(path("/v1/invoices"))
        .and(header("Grpc-Metadata-macaroon", "deadbeef"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "r_hash": hash_b64,
            "payment_request": bolt11_inv,
            "add_index": "1"
        })))
        .mount(&mock_lnd)
        .await;

    let backend = LndBackend::new(mock_lnd.uri(), "deadbeef", Client::new());

    let invoice = backend
        .create_invoice(Satoshis(100), "Inference prompt")
        .await
        .expect("Failed to create invoice");

    assert_eq!(invoice.bolt11, bolt11_inv);
    assert_eq!(invoice.payment_hash.0, hash_hex);
    assert_eq!(invoice.amount, Satoshis(100));
}

#[tokio::test]
async fn test_lnd_backend_is_invoice_settled() {
    let mock_lnd = MockServer::start().await;

    let hash_bytes = [9u8; 32];
    let hash_hex = hex::encode(hash_bytes);

    Mock::given(method("GET"))
        .and(path(format!("/v1/invoice/{}", hash_hex)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "settled": true,
            "state": "SETTLED"
        })))
        .mount(&mock_lnd)
        .await;

    let backend = LndBackend::new(mock_lnd.uri(), "deadbeef", Client::new());

    let settled = backend
        .is_invoice_settled(&PaymentHash(hash_hex))
        .await
        .expect("Failed to check settlement");

    assert!(settled);
}

#[tokio::test]
async fn test_lnd_backend_pay_invoice_success() {
    let mock_lnd = MockServer::start().await;

    let preimage_bytes = [42u8; 32];
    let preimage_b64 = base64::engine::general_purpose::STANDARD.encode(preimage_bytes);
    let preimage_hex = hex::encode(preimage_bytes);

    Mock::given(method("POST"))
        .and(path("/v1/channels/transactions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "payment_preimage": preimage_b64,
            "payment_error": ""
        })))
        .mount(&mock_lnd)
        .await;

    let backend = LndBackend::new(mock_lnd.uri(), "deadbeef", Client::new());

    let preimage = backend
        .pay_invoice("lnbctest100n1...")
        .await
        .expect("Failed to pay invoice");

    assert_eq!(preimage, preimage_hex);
}

#[tokio::test]
async fn test_lnd_backend_missing_config_errors() {
    let config = LightningConfig {
        backend: LightningBackendType::Lnd,
        network: BitcoinNetwork::Testnet,
        lnd_rpc_host: None,
        lnd_macaroon_path: None,
        lnd_tls_cert_path: None,
        nwc_uri: None,
        ..Default::default()
    };

    let result = LndBackend::from_config(&config);
    assert!(result.is_err());
    let err_str = result.err().unwrap().to_string();
    assert!(err_str.contains("Missing lnd_rpc_host") || err_str.contains("configuration"));
}

#[tokio::test]
async fn test_lnd_backend_testnet_flow() {
    let mock_lnd = MockServer::start().await;

    let hash_bytes = [11u8; 32];
    let hash_b64 = base64::engine::general_purpose::STANDARD.encode(hash_bytes);
    let hash_hex = hex::encode(hash_bytes);
    // Standard testnet invoice prefix: lntb
    let testnet_invoice = "lntb10u1pvjql8zpp5...";

    // 1. Mock invoice creation on Testnet LND
    Mock::given(method("POST"))
        .and(path("/v1/invoices"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "r_hash": hash_b64,
            "payment_request": testnet_invoice,
            "add_index": "100"
        })))
        .mount(&mock_lnd)
        .await;

    // 2. Mock invoice lookup on Testnet LND
    Mock::given(method("GET"))
        .and(path(format!("/v1/invoice/{}", hash_hex)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "settled": true,
            "state": "SETTLED"
        })))
        .mount(&mock_lnd)
        .await;

    let backend = LndBackend::new(mock_lnd.uri(), "macaroon_testnet", Client::new());

    // Create testnet invoice
    let invoice = backend
        .create_invoice(Satoshis(10), "Testnet Inference")
        .await
        .expect("Failed to create testnet invoice");

    assert!(invoice.bolt11.starts_with("lntb"));
    assert_eq!(invoice.payment_hash.0, hash_hex);

    // Verify settlement on testnet LND
    let is_settled = backend
        .is_invoice_settled(&invoice.payment_hash)
        .await
        .expect("Failed to check testnet settlement");
    assert!(is_settled);
}

#[tokio::test]
async fn test_lnd_backend_mainnet_flow() {
    let mock_lnd = MockServer::start().await;

    let hash_bytes = [22u8; 32];
    let hash_b64 = base64::engine::general_purpose::STANDARD.encode(hash_bytes);
    let hash_hex = hex::encode(hash_bytes);
    // Standard mainnet invoice prefix: lnbc
    let mainnet_invoice = "lnbc10u1pvjql8zpp5...";

    // 1. Mock invoice creation on Mainnet LND
    Mock::given(method("POST"))
        .and(path("/v1/invoices"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "r_hash": hash_b64,
            "payment_request": mainnet_invoice,
            "add_index": "200"
        })))
        .mount(&mock_lnd)
        .await;

    // 2. Mock invoice lookup on Mainnet LND
    Mock::given(method("GET"))
        .and(path(format!("/v1/invoice/{}", hash_hex)))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "settled": true,
            "state": "SETTLED"
        })))
        .mount(&mock_lnd)
        .await;

    let backend = LndBackend::new(mock_lnd.uri(), "macaroon_mainnet", Client::new());

    // Create mainnet invoice
    let invoice = backend
        .create_invoice(Satoshis(100), "Mainnet Production Inference")
        .await
        .expect("Failed to create mainnet invoice");

    assert!(invoice.bolt11.starts_with("lnbc"));
    assert_eq!(invoice.payment_hash.0, hash_hex);

    // Verify settlement on mainnet LND
    let is_settled = backend
        .is_invoice_settled(&invoice.payment_hash)
        .await
        .expect("Failed to check mainnet settlement");
    assert!(is_settled);
}

#[tokio::test]
async fn test_live_lnd_if_configured() {
    let rest_host = std::env::var("LND_TEST_REST_HOST").ok();
    let macaroon_path = std::env::var("LND_TEST_MACAROON_PATH").ok();
    let cert_path = std::env::var("LND_TEST_TLS_CERT_PATH").ok();

    if let (Some(host), Some(macaroon)) = (rest_host, macaroon_path) {
        println!("Live LND environment detected at {host}! Running live integration test...");

        let config = LightningConfig {
            backend: LightningBackendType::Lnd,
            network: BitcoinNetwork::Testnet,
            lnd_rpc_host: Some(host),
            lnd_macaroon_path: Some(macaroon),
            lnd_tls_cert_path: cert_path,
            nwc_uri: None,
            ..Default::default()
        };

        let backend =
            LndBackend::from_config(&config).expect("Failed to initialize LndBackend from config");

        let invoice = backend
            .create_invoice(Satoshis(10), "Infernos Integration Test")
            .await
            .expect("Failed to create live invoice");

        assert!(!invoice.bolt11.is_empty());
        assert_eq!(invoice.payment_hash.0.len(), 64);

        let is_settled = backend
            .is_invoice_settled(&invoice.payment_hash)
            .await
            .expect("Failed to query live invoice settlement");
        assert!(!is_settled);
    }
}
