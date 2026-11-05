use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use satsgrid::config::schema::{NodeConfig, PricingConfig};
use satsgrid::node::api::routes::create_routes;
use satsgrid::node::api::AppState;
use satsgrid::node::gate::{MacaroonService, SessionBudgetManager};
use satsgrid::node::lightning::mock::MockLightningBackend;
use satsgrid::node::proxy::openai::OpenAiProxy;
use serde_json::json;
use std::sync::{Arc, Mutex};
use tower::ServiceExt; // for `oneshot`
use tower_http::trace::TraceLayer;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone)]
struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SharedBuffer {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn setup_app() -> axum::Router {
    let config = NodeConfig {
        server: satsgrid::config::schema::ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            admin_token: None,
        },
        pricing: PricingConfig::new(satsgrid::common::types::Satoshis(10)),
        upstream: satsgrid::config::schema::UpstreamConfig {
            url: "http://localhost:8080".to_string(),
        },
        lightning: satsgrid::config::schema::LightningConfig::default(),
        data_dir: ".satsgrid_test_data".to_string(),
        ..Default::default()
    };

    let proxy = OpenAiProxy::new(config.upstream.url.clone());

    let config_arc = Arc::new(config);
    let state = AppState {
        config: config_arc.clone(),
        live_pricing: Arc::new(tokio::sync::RwLock::new(config_arc.pricing.clone())),
        lightning: Arc::new(MockLightningBackend::new()),
        budget_manager: Arc::new(SessionBudgetManager::new()),
        macaroon_service: Arc::new(MacaroonService::new(
            b"test-secret-key-0000000000000000".to_vec(),
            "satsgrid-node",
        )),
        proxy,
        stats: Arc::new(satsgrid::node::api::NodeStats::default()),
        admin_token: Arc::new("test-admin-token".to_string()),
    };

    create_routes(state).layer(
        TraceLayer::new_for_http()
            .make_span_with(tower_http::trace::DefaultMakeSpan::new().include_headers(false)),
    )
}

#[tokio::test]
async fn test_privacy_audit_prompt_not_logged() {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let shared_buffer = SharedBuffer(buffer.clone());

    // We set up a local tracing subscriber that writes to our in-memory buffer
    let subscriber = tracing_subscriber::fmt()
        .with_writer(shared_buffer)
        .finish();

    // Use a guard so it only applies to this thread if run concurrently
    let _guard = tracing::subscriber::set_default(subscriber);

    let app = setup_app();

    // This is the highly sensitive prompt that must NOT appear in the logs
    let secret_prompt = "SUPER_SECRET_USER_PROMPT_123456789";

    let req_body = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": secret_prompt}]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&req_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Request will fail with 402 because we aren't sending L402 auth, but tracing still happens.
    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

    // Give it a tiny bit of time just in case some traces flush late
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Check logs
    let logs = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();

    assert!(
        !logs.contains(secret_prompt),
        "PRIVACY VIOLATION: The prompt payload was found in the tracing logs!"
    );
}
