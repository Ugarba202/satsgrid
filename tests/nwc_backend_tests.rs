use clap::Parser;
use satsgrid::client::pay::LightningPaymentProvider;
use satsgrid::config::schema::{LightningBackendType, NodeConfig};
use satsgrid::node::lightning::backend::LightningBackend;
use satsgrid::node::lightning::nwc::NwcLightningBackend;
use std::sync::Arc;

const VALID_NWC_URI: &str = "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=71a8c14c1407c113601079c4302dab36460f0ccd0ad506f1f2dc73b5100e4f3c";

#[test]
fn test_nwc_backend_invalid_uris_rejected() {
    let invalid_cases = vec![
        "",
        "not_a_uri",
        "http://localhost:8080",
        "lightning:lnbc100u...",
        "nostr+walletconnect://",
        "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4", // Missing query params
        "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io", // Missing secret
    ];

    for invalid in invalid_cases {
        let res = NwcLightningBackend::new(invalid.to_string());
        assert!(
            res.is_err(),
            "Expected URI '{}' to fail validation, but it succeeded",
            invalid
        );
    }
}

#[test]
fn test_nwc_backend_valid_uri_initialization() {
    let backend = NwcLightningBackend::new(VALID_NWC_URI.to_string())
        .expect("Valid NWC URI should initialize successfully");

    assert_eq!(
        backend.uri.public_key.to_hex(),
        "b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4"
    );
    assert_eq!(backend.uri.relays[0].as_str(), "wss://relay.damus.io");
}

#[test]
fn test_nwc_backend_implements_lightning_backend_trait() {
    let backend = NwcLightningBackend::new(VALID_NWC_URI.to_string())
        .expect("Valid NWC URI should initialize");

    let trait_object: Arc<dyn LightningBackend> = Arc::new(backend);
    let cloned = trait_object.clone();
    drop(cloned);
}

#[test]
fn test_nwc_backend_implements_payment_provider_trait() {
    let backend = NwcLightningBackend::new(VALID_NWC_URI.to_string())
        .expect("Valid NWC URI should initialize");

    let payment_provider: Arc<dyn LightningPaymentProvider> = Arc::new(backend);
    // Verifies that trait object can be shared across async tasks
    let cloned = payment_provider.clone();
    drop(cloned);
}

#[test]
fn test_node_config_deserializes_nwc_backend() {
    let toml = format!(
        r#"
        [server]
        host = "127.0.0.1"
        port = 8080

        [lightning]
        backend = "nwc"
        nwc_uri = "{}"
        "#,
        VALID_NWC_URI
    );

    let config: NodeConfig = toml::from_str(&toml).expect("NWC node config should deserialize");
    assert_eq!(config.lightning.backend, LightningBackendType::Nwc);
    assert_eq!(config.lightning.nwc_uri.as_deref(), Some(VALID_NWC_URI));
}

#[derive(Parser, Debug)]
struct CliTestWrapper {
    #[command(flatten)]
    call_args: satsgrid::cli::call::CallArgs,
}

#[test]
fn test_cli_call_args_parses_payer_nwc_uri() {
    let args = CliTestWrapper::try_parse_from([
        "infernos",
        "--model",
        "llama3.2",
        "--prompt",
        "Hello",
        "--payer-nwc-uri",
        VALID_NWC_URI,
    ])
    .expect("Cli args with --payer-nwc-uri should parse successfully");

    assert_eq!(args.call_args.payer_nwc_uri.as_deref(), Some(VALID_NWC_URI));
}
