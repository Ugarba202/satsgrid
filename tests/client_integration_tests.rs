use async_trait::async_trait;
use satsgrid::client::budget::ClientBudgetTracker;
use satsgrid::client::error::ClientError;
use satsgrid::client::openai::{ChatCompletionRequest, ChatMessage};
use satsgrid::client::pay::LightningPaymentProvider;
use satsgrid::client::SatsGridClient;
use satsgrid::common::types::{PaymentHash, Satoshis};
use satsgrid::config::schema::{
    LightningConfig, NodeConfig, PricingConfig, ServerConfig, UpstreamConfig,
};
use satsgrid::node::api::routes::create_routes;
use satsgrid::node::api::AppState;
use satsgrid::node::gate::budget::SessionBudgetManager;
use satsgrid::node::gate::macaroon::MacaroonService;
use satsgrid::node::lightning::backend::LightningBackend;
use satsgrid::node::lightning::invoice::Invoice;
use satsgrid::node::proxy::openai::OpenAiProxy;
use serde_json::json;
use std::sync::Arc;
use tokio::net::TcpListener;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A node-side lightning backend that uses a deterministic hash so the client can "pay" it
#[derive(Clone)]
struct DeterministicNodeLightning {
    is_settled: Arc<tokio::sync::Mutex<bool>>,
}

#[async_trait]
impl LightningBackend for DeterministicNodeLightning {
    async fn create_invoice(
        &self,
        amount_sats: Satoshis,
        _memo: &str,
    ) -> satsgrid::common::error::Result<Invoice> {
        let preimage = "0000000000000000000000000000000000000000000000000000000000000000";
        let payment_hash =
            satsgrid::node::gate::verify::L402Verifier::hash_preimage(preimage).unwrap();

        Ok(Invoice {
            bolt11: format!("lnbc{}mock", amount_sats.0),
            payment_hash,
            amount: amount_sats,
        })
    }

    async fn is_invoice_settled(
        &self,
        _payment_hash: &PaymentHash,
    ) -> satsgrid::common::error::Result<bool> {
        let settled = *self.is_settled.lock().await;
        Ok(settled)
    }
}

/// A client-side payment provider that "pays" the deterministic invoice by returning the expected preimage
struct DeterministicClientPaymentProvider {
    node_backend: DeterministicNodeLightning,
}

#[async_trait]
impl LightningPaymentProvider for DeterministicClientPaymentProvider {
    async fn pay_invoice(&self, _invoice: &str) -> Result<String, ClientError> {
        // Mark as settled on the node side!
        *self.node_backend.is_settled.lock().await = true;

        // Return the preimage that hashes to the node's deterministic payment_hash
        Ok("0000000000000000000000000000000000000000000000000000000000000000".to_string())
    }
}

#[tokio::test]
async fn test_end_to_end_client_node_integration() {
    // 1. Setup mock upstream (e.g., Ollama or OpenAI)
    let upstream_mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "llama3.2",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello from the mocked upstream!"
                },
                "finish_reason": "stop"
            }]
        })))
        .mount(&upstream_mock)
        .await;

    // 2. Setup Node Server State
    let node_lightning = DeterministicNodeLightning {
        is_settled: Arc::new(tokio::sync::Mutex::new(false)),
    };

    let config = NodeConfig {
        server: ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            admin_token: None,
            ..Default::default()
        },
        pricing: PricingConfig::new(Satoshis(10)),
        upstream: UpstreamConfig {
            url: upstream_mock.uri(),
            ..Default::default()
        },
        lightning: LightningConfig::default(),
        data_dir: ".satsgrid_test_data".to_string(),
        ..Default::default()
    };

    let state = AppState {
        config: std::sync::Arc::new(config.clone()),
        live_pricing: std::sync::Arc::new(tokio::sync::RwLock::new(config.pricing.clone())),
        lightning: Arc::new(node_lightning.clone()),
        budget_manager: Arc::new(SessionBudgetManager::new()),
        macaroon_service: Arc::new(MacaroonService::new(
            b"test-secret-key-0000000000000000".to_vec(),
            "satsgrid-node",
        )),
        proxy: OpenAiProxy::new(upstream_mock.uri()),
        stats: Arc::new(satsgrid::node::api::NodeStats::default()),
        admin_token: Arc::new("test-admin-token".to_string()),
        db: None,
    };

    let app = create_routes(state);

    // 3. Start Node Server on random port
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let node_url = format!("http://127.0.0.1:{}", port);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // 4. Setup Infernos Client
    let client_provider = Arc::new(DeterministicClientPaymentProvider {
        node_backend: node_lightning,
    });

    let budget_tracker = Arc::new(ClientBudgetTracker::new(100));

    let client = SatsGridClient::builder()
        .node_url(node_url)
        .payment_provider(client_provider)
        .build()
        .unwrap()
        .with_budget(budget_tracker.clone());

    // 5. Create new session
    client
        .create_session(500)
        .await
        .expect("Client should successfully get a challenge, pay it, and cache L402");

    let req = ChatCompletionRequest {
        model: "llama3.2".to_string(),
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "Hello!".to_string(),
        }],
        stream: None,
    };

    let response = client
        .chat(&req)
        .await
        .expect("Client should successfully authorize with cached L402");

    assert_eq!(
        response.choices[0].message.content,
        "Hello from the mocked upstream!"
    );

    // Budget should be decremented by 10 (since default expected price is 10)
    assert_eq!(budget_tracker.remaining(), 90);
}

#[tokio::test]
async fn test_client_pre_flight_budget_rejection() {
    let mock_server = MockServer::start().await;

    // We don't even need the node to respond because the request should be blocked locally.
    // We can just set up a dummy client with a small budget.

    let budget_tracker = Arc::new(ClientBudgetTracker::new(5)); // Only 5 sats!

    let provider = Arc::new(DeterministicClientPaymentProvider {
        node_backend: DeterministicNodeLightning {
            is_settled: Arc::new(tokio::sync::Mutex::new(false)),
        },
    });

    let client = SatsGridClient::builder()
        .node_url(mock_server.uri())
        .payment_provider(provider)
        .build()
        .expect("Client should build");

    let client = client.with_budget(budget_tracker.clone());

    let req = ChatCompletionRequest {
        model: "llama3.2".to_string(),
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "Hello!".to_string(),
        }],
        stream: None,
    };

    let result = client.chat(&req).await;
    assert!(result.is_err());

    match result.unwrap_err() {
        ClientError::BudgetExceeded { allocated, needed } => {
            assert_eq!(allocated, 5);
            assert_eq!(needed, 10);
        }
        _ => panic!("Expected BudgetExceeded error"),
    }

    // Ensure no requests hit the upstream mock server
    let received_requests = mock_server.received_requests().await.unwrap_or(vec![]);
    assert_eq!(
        received_requests.len(),
        0,
        "No requests should have been made to the server"
    );
}

#[tokio::test]
async fn test_client_session_expired_clears_auth() {
    let mock_server = MockServer::start().await;

    // Simulate 402 from the API server (chat completions)
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(402))
        .mount(&mock_server)
        .await;

    let provider = Arc::new(DeterministicClientPaymentProvider {
        node_backend: DeterministicNodeLightning {
            is_settled: Arc::new(tokio::sync::Mutex::new(false)),
        },
    });

    let client = SatsGridClient::builder()
        .node_url(mock_server.uri())
        .payment_provider(provider)
        .build()
        .expect("Client should build");

    // Inject a dummy auth token directly to simulate an existing but expired session
    {
        let mut auth_lock = client.l402_auth.write().unwrap();
        *auth_lock = Some("L402 dummy:dummy".to_string());
    }

    let req = ChatCompletionRequest {
        model: "llama3.2".to_string(),
        messages: vec![],
        stream: None,
    };

    let result = client.chat(&req).await;

    assert!(result.is_err());

    match result.unwrap_err() {
        ClientError::SessionExpired => {}
        _ => panic!("Expected SessionExpired error"),
    }

    // Prove that the client cleared the auth
    let current_auth = client.l402_auth.read().unwrap().clone();
    assert!(
        current_auth.is_none(),
        "Cached auth should be cleared after 402"
    );
}

#[tokio::test]
async fn test_chat_stream_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string("chunk1chunk2"))
        .mount(&mock_server)
        .await;

    let provider = Arc::new(DeterministicClientPaymentProvider {
        node_backend: DeterministicNodeLightning {
            is_settled: Arc::new(tokio::sync::Mutex::new(false)),
        },
    });

    let client = SatsGridClient::builder()
        .node_url(mock_server.uri())
        .payment_provider(provider)
        .build()
        .expect("Client should build");

    {
        let mut auth_lock = client.l402_auth.write().unwrap();
        *auth_lock = Some("L402 valid".to_string());
    }

    let req = ChatCompletionRequest {
        model: "llama3.2".to_string(),
        messages: vec![],
        stream: None,
    };

    let mut stream = client.chat_stream(&req).await.unwrap();

    use futures_util::StreamExt;
    let mut received = Vec::new();
    while let Some(chunk) = stream.next().await {
        received.extend_from_slice(&chunk.unwrap());
    }

    let received_str = String::from_utf8(received).unwrap();
    assert_eq!(received_str, "chunk1chunk2");
}

#[tokio::test]
async fn test_chat_stream_session_expired_clears_auth() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(402))
        .mount(&mock_server)
        .await;

    let provider = Arc::new(DeterministicClientPaymentProvider {
        node_backend: DeterministicNodeLightning {
            is_settled: Arc::new(tokio::sync::Mutex::new(false)),
        },
    });

    let client = SatsGridClient::builder()
        .node_url(mock_server.uri())
        .payment_provider(provider)
        .build()
        .unwrap();

    {
        let mut auth_lock = client.l402_auth.write().unwrap();
        *auth_lock = Some("L402 dummy".to_string());
    }

    let req = ChatCompletionRequest {
        model: "llama".to_string(),
        messages: vec![],
        stream: None,
    };

    let result = client.chat_stream(&req).await;
    match result {
        Err(ClientError::SessionExpired) => {}
        _ => panic!("Expected SessionExpired error"),
    }

    let current_auth = client.l402_auth.read().unwrap().clone();
    assert!(current_auth.is_none());
}
