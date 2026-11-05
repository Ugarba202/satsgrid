use async_trait::async_trait;
use satsgrid::client::error::ClientError;
use satsgrid::client::pay::LightningPaymentProvider;
use satsgrid::client::SatsGridClient;

struct DummyLightningProvider;

#[async_trait]
impl LightningPaymentProvider for DummyLightningProvider {
    async fn pay_invoice(&self, _invoice: &str) -> Result<String, ClientError> {
        Ok("dummy".to_string())
    }
}

#[test]
fn test_client_builder_missing_node_url() {
    let provider = std::sync::Arc::new(DummyLightningProvider);

    // Attempting to build without node URL should fail
    let result = SatsGridClient::builder().payment_provider(provider).build();

    assert!(result.is_err());
}

#[test]
fn test_client_builder_missing_provider() {
    // Attempting to build without payment provider should fail
    let result = SatsGridClient::builder()
        .node_url("http://127.0.0.1:8080".to_string())
        .build();

    assert!(result.is_err());
}

#[test]
fn test_client_builder_success() {
    let provider = std::sync::Arc::new(DummyLightningProvider);

    // Providing both should succeed
    let result = SatsGridClient::builder()
        .node_url("http://127.0.0.1:8080".to_string())
        .payment_provider(provider)
        .build();

    assert!(result.is_ok());
}
