use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use satsgrid::common::types::{Satoshis, SessionId};
use satsgrid::config::schema::{NodeConfig, PricingConfig, ServerConfig, UpstreamConfig};
use satsgrid::node::api::routes::create_routes;
use satsgrid::node::api::AppState;
use satsgrid::node::gate::{Caveat, MacaroonService, SessionBudgetManager};
use satsgrid::node::lightning::mock::MockLightningBackend;
use satsgrid::node::lightning::LightningBackend;
use satsgrid::node::proxy::openai::OpenAiProxy;
use satsgrid::node::storage::Database;
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::tempdir;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct TestContext {
    app: axum::Router,
    db: Arc<Database>,
    lightning: Arc<MockLightningBackend>,
    macaroon_service: Arc<MacaroonService>,
    budget_manager: Arc<SessionBudgetManager>,
    _dir: tempfile::TempDir,
    _mock_server: MockServer,
}

async fn setup_test_context(max_bytes: usize, max_completion_tokens: usize) -> TestContext {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "llama3.2",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "Hello!" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 5, "completion_tokens": 5, "total_tokens": 10 }
        })))
        .mount(&mock_server)
        .await;

    let config = NodeConfig {
        server: ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            admin_token: Some("test-admin-token".to_string()),
            max_request_body_bytes: max_bytes,
            max_completion_tokens_limit: max_completion_tokens,
        },
        pricing: PricingConfig::new(Satoshis(10)),
        upstream: UpstreamConfig {
            url: mock_server.uri(),
            ..Default::default()
        },
        data_dir: dir.path().to_string_lossy().to_string(),
        ..Default::default()
    };

    let lightning = Arc::new(MockLightningBackend::new());
    let macaroon_service = Arc::new(MacaroonService::new([7u8; 32].to_vec(), "satsgrid-test"));
    let budget_manager = Arc::new(SessionBudgetManager::with_store(db.session_store()));
    let proxy = OpenAiProxy::new(config.upstream.url.clone());

    let state = AppState {
        config: Arc::new(config.clone()),
        live_pricing: Arc::new(tokio::sync::RwLock::new(config.pricing.clone())),
        lightning: lightning.clone(),
        budget_manager: budget_manager.clone(),
        macaroon_service: macaroon_service.clone(),
        proxy,
        stats: Arc::new(satsgrid::node::api::NodeStats::default()),
        admin_token: Arc::new("test-admin-token".to_string()),
        db: Some(db.clone()),
    };

    let app = create_routes(state);

    TestContext {
        app,
        db,
        lightning,
        macaroon_service,
        budget_manager,
        _dir: dir,
        _mock_server: mock_server,
    }
}

#[tokio::test]
async fn test_request_body_exceeds_content_length_limit() {
    let ctx = setup_test_context(1024, 128_000).await; // 1 KB limit

    let oversized_padding = "x".repeat(2048);
    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": oversized_padding}]
    });
    let body_bytes = serde_json::to_vec(&payload).unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, body_bytes.len())
        .body(Body::from(body_bytes))
        .unwrap();

    let resp = ctx.app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let err_json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(err_json["error"]["type"], "invalid_request_error");
    assert_eq!(err_json["error"]["code"], "request_too_large");
}

#[tokio::test]
async fn test_chunked_streaming_request_exceeds_limit_without_content_length() {
    let ctx = setup_test_context(1024, 128_000).await; // 1 KB limit

    // Stream 3 chunks of 500 bytes each (total 1500 bytes > 1024 limit)
    let chunks = vec![
        Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![b' '; 500])),
        Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![b' '; 500])),
        Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![b' '; 500])),
    ];
    let stream = futures_util::stream::iter(chunks);
    let body = Body::from_stream(stream);

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        // No Content-Length header!
        .body(body)
        .unwrap();

    let resp = ctx.app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let err_json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(err_json["error"]["type"], "invalid_request_error");
    assert_eq!(err_json["error"]["code"], "request_too_large");
}

#[tokio::test]
async fn test_malformed_json_syntax_returns_400_openai_format() {
    let ctx = setup_test_context(1024 * 1024, 128_000).await;

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{ malformed json }"))
        .unwrap();

    let resp = ctx.app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let err_json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(err_json["error"]["type"], "invalid_request_error");
    assert_eq!(err_json["error"]["code"], "invalid_json");
}

#[tokio::test]
async fn test_pre_auth_validation_failure_returns_400_not_402() {
    let ctx = setup_test_context(1024 * 1024, 128_000).await;

    // Missing messages array
    let payload = json!({
        "model": "llama3.2"
    });

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        // No Authorization header
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp = ctx.app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let err_json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(err_json["error"]["type"], "invalid_request_error");
    assert_eq!(err_json["error"]["param"], "messages");
}

#[tokio::test]
async fn test_valid_request_without_auth_returns_402_challenge() {
    let ctx = setup_test_context(1024 * 1024, 128_000).await;

    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hello"}]
    });

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp = ctx.app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PAYMENT_REQUIRED);
    assert!(resp.headers().contains_key("www-authenticate"));
}

#[tokio::test]
async fn test_invalid_request_with_valid_payment_does_not_consume_payment_hash() {
    let ctx = setup_test_context(1024 * 1024, 128_000).await;

    // Mint and pay for an invoice
    let invoice = ctx
        .lightning
        .create_invoice(Satoshis(10), "Test")
        .await
        .unwrap();
    let preimage = ctx.lightning.pay_invoice(&invoice.bolt11).await.unwrap();
    let macaroon = ctx
        .macaroon_service
        .mint(
            &invoice.payment_hash,
            vec![
                Caveat::Capability("inference".to_string()),
                Caveat::ExpiresAt((chrono::Utc::now().timestamp() + 3600) as u64),
                Caveat::Model("llama3.2".to_string()),
            ],
        )
        .unwrap();

    let auth_header = format!("L402 {}:{}", macaroon.to_base64().unwrap(), preimage);

    // Invalid payload: out-of-range temperature
    let invalid_payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hello"}],
        "temperature": 5.0 // Invalid! max is 2.0
    });

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, &auth_header)
        .body(Body::from(serde_json::to_vec(&invalid_payload).unwrap()))
        .unwrap();

    let resp = ctx.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Verify: payment hash was NEVER reserved or consumed in payment_authorizations table!
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM payment_authorizations WHERE payment_hash = ?1")
            .bind(&invoice.payment_hash.0)
            .fetch_one(ctx.db.pool())
            .await
            .unwrap();
    assert_eq!(
        count, 0,
        "Payment hash must NOT be consumed or reserved on invalid request"
    );

    // Verify: no inference_requests record was created
    let count_req: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inference_requests")
        .fetch_one(ctx.db.pool())
        .await
        .unwrap();
    assert_eq!(
        count_req, 0,
        "No inference_requests record should be created for rejected request"
    );

    // Verify: since payment hash was NOT consumed, a corrected valid request now succeeds!
    let valid_payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hello"}],
        "temperature": 0.7
    });

    let valid_req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, &auth_header)
        .body(Body::from(serde_json::to_vec(&valid_payload).unwrap()))
        .unwrap();

    let valid_resp = ctx.app.oneshot(valid_req).await.unwrap();
    assert_eq!(valid_resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_invalid_request_with_valid_session_does_not_debit_budget() {
    let ctx = setup_test_context(1024 * 1024, 128_000).await;

    // Create a funded session
    let session_uuid = uuid::Uuid::new_v4();
    let session_id = SessionId(session_uuid);
    let invoice = ctx
        .lightning
        .create_invoice(Satoshis(100), "Session")
        .await
        .unwrap();
    let preimage = ctx.lightning.pay_invoice(&invoice.bolt11).await.unwrap();

    ctx.budget_manager
        .open_session_with_details(
            &session_id,
            Some(&invoice.payment_hash),
            Satoshis(100),
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .unwrap();

    let macaroon = ctx
        .macaroon_service
        .mint(
            &invoice.payment_hash,
            vec![
                Caveat::Session(session_uuid.to_string()),
                Caveat::Budget(100),
                Caveat::Capability("inference".to_string()),
            ],
        )
        .unwrap();

    let auth_header = format!("L402 {}:{}", macaroon.to_base64().unwrap(), preimage);

    // Initial session balance
    let initial_session = ctx
        .db
        .session_store()
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(initial_session.remaining_sats, Satoshis(100));

    // Submit invalid request: negative max_tokens
    let invalid_payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": -10
    });

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, &auth_header)
        .body(Body::from(serde_json::to_vec(&invalid_payload).unwrap()))
        .unwrap();

    let resp = ctx.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Verify session budget is completely untouched
    let after_session = ctx
        .db
        .session_store()
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_session.remaining_sats, Satoshis(100));
    assert_eq!(after_session.reserved_sats, Satoshis(0));

    // Verify no reservation records exist
    let res_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM session_reservations WHERE session_id = ?1")
            .bind(session_id.0.to_string())
            .fetch_one(ctx.db.pool())
            .await
            .unwrap();
    assert_eq!(res_count, 0);
}

#[tokio::test]
async fn test_schema_model_validation() {
    let ctx = setup_test_context(1024 * 1024, 128_000).await;

    // 1. Empty model string
    let payload = json!({
        "model": "   ",
        "messages": [{"role": "user", "content": "hi"}]
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();
    let resp = ctx.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // 2. Model with invalid characters
    let payload = json!({
        "model": "llama3.2; DROP TABLE inference_requests;--",
        "messages": [{"role": "user", "content": "hi"}]
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();
    let resp = ctx.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // 3. Model exceeds 128 characters
    let payload = json!({
        "model": "a".repeat(129),
        "messages": [{"role": "user", "content": "hi"}]
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();
    let resp = ctx.app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_schema_messages_and_roles_validation() {
    let ctx = setup_test_context(1024 * 1024, 128_000).await;

    // 1. Empty messages array
    let payload = json!({
        "model": "llama3.2",
        "messages": []
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();
    let resp = ctx.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // 2. Unsupported role
    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "admin", "content": "hi"}]
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();
    let resp = ctx.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // 3. Null content for non-assistant role
    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": null}]
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();
    let resp = ctx.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // 4. Tool role without tool_call_id
    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "tool", "content": "result"}]
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();
    let resp = ctx.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // 5. Assistant with tool_calls and null content (VALID per OpenAI spec)
    let payload = json!({
        "model": "llama3.2",
        "messages": [
            {"role": "user", "content": "what is weather?"},
            {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_123",
                    "type": "function",
                    "function": {"name": "get_weather", "arguments": "{}"}
                }]
            },
            {
                "role": "tool",
                "tool_call_id": "call_123",
                "content": "72F and sunny"
            }
        ]
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();
    let resp = ctx.app.oneshot(req).await.unwrap();
    // Reaches payment challenge (valid syntax & roles)
    assert_eq!(resp.status(), StatusCode::PAYMENT_REQUIRED);
}

#[tokio::test]
async fn test_numeric_parameters_boundary_validation() {
    let ctx = setup_test_context(1024 * 1024, 4096).await; // 4096 max completion tokens limit

    // 1. temperature < 0.0
    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hi"}],
        "temperature": -0.1
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();
    assert_eq!(
        ctx.app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );

    // 2. top_p > 1.0
    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hi"}],
        "top_p": 1.01
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();
    assert_eq!(
        ctx.app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );

    // 3. presence_penalty < -2.0
    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hi"}],
        "presence_penalty": -2.05
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();
    assert_eq!(
        ctx.app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );

    // 4. max_tokens exceeds node limit
    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 5000 // Limit is 4096
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();
    assert_eq!(
        ctx.app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );

    // 5. max_completion_tokens takes precedence over max_tokens
    // If max_tokens is within limit (100) but max_completion_tokens exceeds limit (5000), it must reject
    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hi"}],
        "max_tokens": 100,
        "max_completion_tokens": 5000
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();
    assert_eq!(
        ctx.app.oneshot(req).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
}
