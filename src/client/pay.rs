use crate::client::error::ClientError;
use crate::node::gate::challenge::L402Challenge;
use async_trait::async_trait;
use std::sync::Arc;

/// An abstraction for a lightning payment provider on the caller side.
/// Implement this trait to provide concrete integration with LND, Core Lightning, etc.
#[async_trait]
pub trait LightningPaymentProvider: Send + Sync {
    /// Pays a bolt11 invoice and returns the preimage as a hex string.
    async fn pay_invoice(&self, invoice: &str) -> Result<String, ClientError>;
}

/// Automatically handles L402 `402 Payment Required` challenges by parsing
/// the WWW-Authenticate header, paying the extracted invoice using the
/// provided Lightning backend, and constructing the L402 Authorization header.
pub struct L402PaymentHandler {
    provider: Arc<dyn LightningPaymentProvider>,
}

impl L402PaymentHandler {
    pub fn new(provider: Arc<dyn LightningPaymentProvider>) -> Self {
        Self { provider }
    }

    /// Handles an incoming 402 challenge header (e.g., from `WWW-Authenticate`),
    /// processes the payment, and returns the constructed `Authorization` header value.
    pub async fn handle_challenge(&self, challenge_header: &str) -> Result<String, ClientError> {
        // Parse the challenge using the shared protocol definition to avoid duplication
        let challenge = L402Challenge::from_header_value(challenge_header)
            .map_err(|e| ClientError::MalformedChallenge(e.to_string()))?;

        // Pay the invoice using the injected payment provider
        let preimage = self.provider.pay_invoice(&challenge.invoice).await?;

        // Construct the L402 credentials exactly per protocol
        Ok(format!("L402 {}:{}", challenge.macaroon, preimage))
    }
}
