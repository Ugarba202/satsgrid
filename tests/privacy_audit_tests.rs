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
            ..Default::default()
        },
        pricing: PricingConfig::new(satsgrid::common::types::Satoshis(10)),
        upstream: satsgrid::config::schema::UpstreamConfig {
            url: "http://localhost:8080".to_string(),
            ..Default::default()
        },
        lightning: satsgrid::config::schema::LightningConfig {
            allow_mock: true,
            ..Default::default()
        },
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
        db: None,
        concurrency_manager: Arc::new(satsgrid::node::concurrency::ConcurrencyManager::new(
            config_arc.concurrency.max_concurrent_requests,
            config_arc.concurrency.max_concurrent_per_session,
            config_arc.concurrency.admission_timeout_secs,
        )),
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

#[tokio::test]
async fn test_privacy_audit_l402_credentials_not_logged_or_returned() {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let shared_buffer = SharedBuffer(buffer.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_writer(shared_buffer)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let app = setup_app();
    let fake_macaroon = "FAKE_MACAROON_CREDENTIAL_MARKER_41A7";
    let fake_preimage = "FAKE_PAYMENT_PREIMAGE_MARKER_9C32";
    let authorization = format!("L402 {fake_macaroon}:{fake_preimage}");
    let request_body = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "credential privacy check"}]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("Content-Type", "application/json")
                .header("Authorization", &authorization)
                .body(Body::from(serde_json::to_vec(&request_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response_body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let response_text = String::from_utf8_lossy(&response_body);
    assert!(!response_text.contains(fake_macaroon));
    assert!(!response_text.contains(fake_preimage));

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let logs = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
    assert!(!logs.contains(fake_macaroon));
    assert!(!logs.contains(fake_preimage));
    assert!(!logs.contains(&authorization));
}
