use crate::client::budget::ClientBudgetTracker;
use crate::client::error::ClientError;
use crate::client::openai::{ChatCompletionRequest, ChatCompletionResponse};
use crate::client::pay::{L402PaymentHandler, LightningPaymentProvider};
use bytes::Bytes;
use futures_util::stream::Stream;
use reqwest::{header, Client};
use serde_json::json;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::RwLock;

pub type ChatStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;

pub struct SatsGridClient {
    pub node_url: String,
    pub budget_tracker: Option<Arc<ClientBudgetTracker>>,
    http_client: Client,
    payment_handler: L402PaymentHandler,

    /// Cached L402 authorization header
    pub l402_auth: RwLock<Option<String>>,
}

pub struct SatsGridClientBuilder {
    node_url: Option<String>,
    payment_provider: Option<Arc<dyn LightningPaymentProvider>>,
}

impl Default for SatsGridClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SatsGridClientBuilder {
    pub fn new() -> Self {
        Self {
            node_url: None,
            payment_provider: None,
        }
    }

    pub fn node_url(mut self, url: String) -> Self {
        self.node_url = Some(url);
        self
    }

    pub fn payment_provider(mut self, provider: Arc<dyn LightningPaymentProvider>) -> Self {
        self.payment_provider = Some(provider);
        self
    }

    pub fn build(self) -> Result<SatsGridClient, ClientError> {
        let node_url = self
            .node_url
            .ok_or_else(|| ClientError::Protocol("Missing node URL".to_string()))?;
        let payment_provider = self
            .payment_provider
            .ok_or_else(|| ClientError::Protocol("Missing payment provider".to_string()))?;

        let payment_handler = L402PaymentHandler::new(payment_provider.clone());

        Ok(SatsGridClient {
            node_url,
            budget_tracker: None,
            http_client: Client::new(),
            payment_handler,
            l402_auth: RwLock::new(None),
        })
    }
}

impl SatsGridClient {
    pub fn builder() -> SatsGridClientBuilder {
        SatsGridClientBuilder::new()
    }

    pub fn with_budget(mut self, tracker: Arc<ClientBudgetTracker>) -> Self {
        self.budget_tracker = Some(tracker);
        self
    }

    /// Requests a new session from the node, pays the 402 challenge, and caches the L402 credentials.
    pub async fn create_session(&self, session_budget_sats: u64) -> Result<(), ClientError> {
        let url = format!("{}/v1/session/new", self.node_url.trim_end_matches('/'));

        let response = self
            .http_client
            .post(&url)
            .json(&json!({ "budget_sats": session_budget_sats }))
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::PAYMENT_REQUIRED {
            if let Some(auth_header) = response.headers().get(header::WWW_AUTHENTICATE) {
                let challenge_str = auth_header.to_str().map_err(|_| {
                    ClientError::Protocol("Invalid WWW-Authenticate header".to_string())
                })?;

                // Pay invoice using handler
                let l402_auth = self.payment_handler.handle_challenge(challenge_str).await?;

                // Cache it
                *self.l402_auth.write().unwrap() = Some(l402_auth);
                return Ok(());
            } else {
                return Err(ClientError::Protocol(
                    "402 response missing WWW-Authenticate header".to_string(),
                ));
            }
        }

        Err(ClientError::Protocol(format!(
            "Expected 402 Payment Required for new session, got {}",
            response.status()
        )))
    }

    pub async fn chat(
        &self,
        req: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ClientError> {
        // Pre-flight check: assume 10 sats cost if not dynamic yet
        let expected_cost_sats = 10;
        if let Some(tracker) = &self.budget_tracker {
            tracker.verify_can_afford(expected_cost_sats)?;
        }

        let url = format!(
            "{}/v1/chat/completions",
            self.node_url.trim_end_matches('/')
        );

        // We assume create_session was called or we have cached auth
        let auth = { self.l402_auth.read().unwrap().clone() };

        let mut request_builder = self.http_client.post(&url).json(req);

        if let Some(l402) = auth {
            request_builder = request_builder.header(header::AUTHORIZATION, l402);
        }

        let response = request_builder.send().await?;

        // If payment is required (budget exhausted on node side, etc.)
        if response.status() == reqwest::StatusCode::PAYMENT_REQUIRED {
            // Note: chat/completions doesn't return a new WWW-Authenticate header in this design
            // Instead we would have to create a new session. We return an error telling caller to do so.

            // Clear the stale cached authorization
            *self.l402_auth.write().unwrap() = None;

            return Err(ClientError::SessionExpired);
        }

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(ClientError::Protocol(format!(
                "Server returned {}: {}",
                status, text
            )));
        }

        if let Some(tracker) = &self.budget_tracker {
            let charged_sats = response
                .headers()
                .get("X-SatsGrid-Charged-Sats")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(expected_cost_sats);

            tracker.spend(charged_sats)?;
        }

        let chat_resp = response.json::<ChatCompletionResponse>().await?;
        Ok(chat_resp)
    }

    pub async fn chat_stream(
        &self,
        req: &ChatCompletionRequest,
    ) -> Result<ChatStream, ClientError> {
        let expected_cost_sats = 10;
        if let Some(tracker) = &self.budget_tracker {
            tracker.verify_can_afford(expected_cost_sats)?;
        }

        let url = format!(
            "{}/v1/chat/completions",
            self.node_url.trim_end_matches('/')
        );

        let mut req_clone = req.clone();
        req_clone.stream = Some(true);

        let auth = { self.l402_auth.read().unwrap().clone() };

        let mut request_builder = self.http_client.post(&url).json(&req_clone);

        if let Some(l402) = auth {
            request_builder = request_builder.header(header::AUTHORIZATION, l402);
        }

        let response = request_builder.send().await?;

        if response.status() == reqwest::StatusCode::PAYMENT_REQUIRED {
            *self.l402_auth.write().unwrap() = None;
            return Err(ClientError::SessionExpired);
        }

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(ClientError::Protocol(format!(
                "Server returned {}: {}",
                status, text
            )));
        }

        if let Some(tracker) = &self.budget_tracker {
            let charged_sats = response
                .headers()
                .get("X-SatsGrid-Charged-Sats")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(expected_cost_sats);

            tracker.spend(charged_sats)?;
        }

        Ok(Box::pin(response.bytes_stream()))
    }
}
