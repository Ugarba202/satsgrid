use satsgrid::common::types::Satoshis;
use satsgrid::config::schema::LightningBackendType;
use satsgrid::config::{ClientConfig, NodeConfig};

/// Verify that a fully specified structured TOML string correctly
/// deserializes into typed sub-sections (`[server]`, `[upstream]`, `[pricing]`, `[lightning]`).
///
/// NOTE: This is a pure in-memory text parsing unit test. No network connections
/// or sockets are created. Standard local loopback addresses (`127.0.0.1`) are used.
#[test]
fn node_config_deserializes_from_structured_toml() {
    let toml = r#"
        [server]
        host = "127.0.0.1"
        port = 9000

        [upstream]
        url = "http://127.0.0.1:11434"

        [pricing]
        default_price_sats = 25

        [lightning]
        backend = "lnd"
        lnd_rpc_host = "127.0.0.1:10009"
    "#;

    let config: NodeConfig =
        toml::from_str(toml).expect("structured node config should deserialize");

    assert_eq!(config.server.host, "127.0.0.1");
    assert_eq!(config.server.port, 9000);
    assert_eq!(config.upstream.url, "http://127.0.0.1:11434");
    assert_eq!(config.pricing.default_price_sats, Satoshis(25));
    assert_eq!(config.lightning.backend, LightningBackendType::Lnd);
    assert_eq!(
        config.lightning.lnd_rpc_host.as_deref(),
        Some("127.0.0.1:10009")
    );
}

/// Verify that when an operator only customizes one section (e.g., `[pricing]`),
/// all omitted sections (`[server]`, `[upstream]`, `[lightning]`) automatically fall back
/// to safe, production-standard defaults without requiring redundant configuration.
#[test]
fn node_config_partial_override_falls_back_to_defaults() {
    let toml = r#"
        [pricing]
        default_price_sats = 100
    "#;

    let config: NodeConfig = toml::from_str(toml).expect("partial node config should deserialize");

    assert_eq!(config.pricing.default_price_sats, Satoshis(100));
    assert_eq!(config.server.host, "0.0.0.0");
    assert_eq!(config.server.port, 8080);
    assert_eq!(config.upstream.url, "http://127.0.0.1:11434");
    assert_eq!(config.lightning.backend, LightningBackendType::Mock);
}

/// Test 3: Verify that a client / agent configuration with structured sections
/// (`[client]`, `[budget]`, `[lightning]`) deserializes properly into `ClientConfig`.
#[test]
fn client_config_deserializes_from_structured_toml() {
    let toml = r#"
        [client]
        node_url = "http://127.0.0.1:8080"

        [budget]
        max_budget_sats = 5000

        [lightning]
        payment_backend = "nwc"
    "#;

    let config: ClientConfig =
        toml::from_str(toml).expect("structured client config should deserialize");

    assert_eq!(config.client.node_url, "http://127.0.0.1:8080");
    assert_eq!(config.budget.max_budget_sats, Some(Satoshis(5000)));
    assert_eq!(config.lightning.payment_backend, LightningBackendType::Nwc);
}

/// Test 4: Verify that when a client only specifies a budget override,
/// the client endpoint and payment backend default to standard local testing values.
#[test]
fn client_config_partial_override_falls_back_to_defaults() {
    let toml = r#"
        [budget]
        max_budget_sats = 250
    "#;

    let config: ClientConfig =
        toml::from_str(toml).expect("partial client config should deserialize");

    assert_eq!(config.client.node_url, "http://127.0.0.1:8080");
    assert_eq!(config.budget.max_budget_sats, Some(Satoshis(250)));
    assert_eq!(config.lightning.payment_backend, LightningBackendType::Mock);
}

/// Test 5: Verify that unrecognized payment rails (e.g. non-crypto or unsupported systems)
/// are rejected at the configuration parsing layer, preventing runtime failures.
#[test]
fn lightning_backend_type_rejects_invalid_variant() {
    let toml = r#"
        [lightning]
        backend = "unsupported_rail"
    "#;

    let result: Result<NodeConfig, _> = toml::from_str(toml);
    assert!(
        result.is_err(),
        "unsupported backend rail must fail to parse"
    );
}
