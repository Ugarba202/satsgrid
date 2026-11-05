use async_trait::async_trait;
use satsgrid::client::error::ClientError;
use satsgrid::client::pay::{L402PaymentHandler, LightningPaymentProvider};
use std::sync::Arc;
use tokio::sync::Mutex;

/// A simple mock lightning provider that records if it was called and returns a fixed preimage.
pub struct MockLightningPaymentProvider {
    pub called_with_invoice: Arc<Mutex<Option<String>>>,
    pub return_preimage: Result<String, String>, // Ok(preimage) or Err(error_message)
}

impl MockLightningPaymentProvider {
    pub fn new_success(preimage: &str) -> Self {
        Self {
            called_with_invoice: Arc::new(Mutex::new(None)),
            return_preimage: Ok(preimage.to_string()),
        }
    }

    pub fn new_failure(error: &str) -> Self {
        Self {
            called_with_invoice: Arc::new(Mutex::new(None)),
            return_preimage: Err(error.to_string()),
        }
    }
}

#[async_trait]
impl LightningPaymentProvider for MockLightningPaymentProvider {
    async fn pay_invoice(&self, invoice: &str) -> Result<String, ClientError> {
        let mut called = self.called_with_invoice.lock().await;
        *called = Some(invoice.to_string());

        match &self.return_preimage {
            Ok(preimage) => Ok(preimage.clone()),
            Err(e) => Err(ClientError::Payment(e.clone())),
        }
    }
}

#[tokio::test]
async fn test_payment_handler_valid_challenge_success() {
    let provider = Arc::new(MockLightningPaymentProvider::new_success("mockpreimage123"));
    let handler = L402PaymentHandler::new(provider.clone());

    let header_value = "L402 token=\"mockmacaroonbase64\", invoice=\"lnbc1mockinvoice\"";

    // When the handler processes the 402 challenge
    let auth_header = handler
        .handle_challenge(header_value)
        .await
        .expect("handler should succeed");

    // It should have extracted the invoice and called the payment provider
    let called_invoice = provider
        .called_with_invoice
        .lock()
        .await
        .clone()
        .expect("provider should have been called");
    assert_eq!(called_invoice, "lnbc1mockinvoice");

    // It should construct exactly the correct L402 Authorization header
    assert_eq!(auth_header, "L402 mockmacaroonbase64:mockpreimage123");
}

#[tokio::test]
async fn test_payment_handler_malformed_challenge_rejected() {
    let provider = Arc::new(MockLightningPaymentProvider::new_success("mockpreimage123"));
    let handler = L402PaymentHandler::new(provider);

    let malformed_header = "L402 invoice=\"lnbc1mockinvoice\""; // missing token

    let result = handler.handle_challenge(malformed_header).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        ClientError::MalformedChallenge(_) => {} // Expected
        other => panic!("Expected MalformedChallenge error, got {:?}", other),
    }
}

#[tokio::test]
async fn test_payment_handler_payment_failure_propagated() {
    let provider = Arc::new(MockLightningPaymentProvider::new_failure("route not found"));
    let handler = L402PaymentHandler::new(provider.clone());

    let header_value = "L402 token=\"mockmacaroonbase64\", invoice=\"lnbc1mockinvoice\"";

    let result = handler.handle_challenge(header_value).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        ClientError::Payment(msg) => {
            assert!(msg.contains("route not found"));
        }
        other => panic!("Expected Payment error, got {:?}", other),
    }

    // Ensure it still attempted the payment
    let called_invoice = provider
        .called_with_invoice
        .lock()
        .await
        .clone()
        .expect("provider should have been called");
    assert_eq!(called_invoice, "lnbc1mockinvoice");
}
