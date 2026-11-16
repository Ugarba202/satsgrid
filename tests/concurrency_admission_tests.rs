use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use satsgrid::common::types::{Satoshis, SessionId};
use satsgrid::config::schema::{ConcurrencyConfig, NodeConfig, PricingConfig, UpstreamConfig};
use satsgrid::node::api::routes::create_routes;
use satsgrid::node::api::AppState;
use satsgrid::node::concurrency::ConcurrencyManager;
use satsgrid::node::gate::macaroon::Caveat;
use satsgrid::node::gate::{MacaroonService, SessionBudgetManager};
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
    mock_server: MockServer,
    concurrency_manager: Arc<ConcurrencyManager>,
}

async fn setup_test_context(concurrency_cfg: ConcurrencyConfig) -> TestContext {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));
    let mock_server = MockServer::start().await;

    let config = NodeConfig {
        upstream: UpstreamConfig {
            url: mock_server.uri(),
            connect_timeout_secs: 2,
            response_header_timeout_secs: 5,
            stream_idle_timeout_secs: 5,
            total_request_timeout_secs: 10,
        },
        pricing: PricingConfig::new(Satoshis(10)),
        concurrency: concurrency_cfg.clone(),
        data_dir: dir.path().to_string_lossy().to_string(),
        ..Default::default()
    };

    let lightning = Arc::new(MockLightningBackend::new());
    let macaroon_service = Arc::new(MacaroonService::new([7u8; 32].to_vec(), "satsgrid-test"));
    let budget_manager = Arc::new(SessionBudgetManager::with_store(db.session_store()));
    let proxy = OpenAiProxy::new(config.upstream.url.clone());

    let concurrency_manager = Arc::new(ConcurrencyManager::new(
        concurrency_cfg.max_concurrent_requests,
        concurrency_cfg.max_concurrent_per_session,
        concurrency_cfg.admission_timeout_secs,
    ));

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
        concurrency_manager: concurrency_manager.clone(),
    };

    let app = create_routes(state);

    TestContext {
        app,
        db,
        lightning,
        macaroon_service,
        budget_manager,
        _dir: dir,
        mock_server,
        concurrency_manager,
    }
}

async fn fund_session(ctx: &TestContext, sats: u64) -> (SessionId, String) {
    let session_uuid = uuid::Uuid::new_v4();
    let session_id = SessionId(session_uuid);
    let invoice = ctx
        .lightning
        .create_invoice(Satoshis(sats), "Session Fund")
        .await
        .unwrap();
    let preimage = ctx.lightning.pay_invoice(&invoice.bolt11).await.unwrap();

    ctx.budget_manager
        .open_session_with_details(
            &session_id,
            Some(&invoice.payment_hash),
            Satoshis(sats),
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
                Caveat::Budget(sats),
                Caveat::Capability("inference".to_string()),
            ],
        )
        .unwrap();

    let auth_header = format!("L402 {}:{}", macaroon.to_base64().unwrap(), preimage);
    (session_id, auth_header)
}

async fn create_pay_per_request_auth(ctx: &TestContext) -> (String, String) {
    let invoice = ctx
        .lightning
        .create_invoice(Satoshis(10), "Inference Payment")
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
            ],
        )
        .unwrap();

    let auth_header = format!("L402 {}:{}", macaroon.to_base64().unwrap(), preimage);
    (invoice.payment_hash.0, auth_header)
}

#[tokio::test]
async fn test_global_saturation_rejects_before_financial_reservation() {
    let ctx = setup_test_context(ConcurrencyConfig {
        max_concurrent_requests: 1,
        max_concurrent_per_session: 1,
        admission_timeout_secs: 0,
    })
    .await;

    // Manually acquire the single global permit to simulate saturation
    let _occupying_permit = ctx.concurrency_manager.try_acquire_all_now(None).unwrap();

    // Now send an inference request with valid pay-per-request payment
    let (pay_hash, auth_header) = create_pay_per_request_auth(&ctx).await;

    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": false
    });

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, &auth_header)
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp = ctx.app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(resp.headers()["retry-after"], "1");

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json_resp: Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(json_resp["error"]["type"], "server_overload_error");
    assert_eq!(json_resp["error"]["code"], "node_capacity_exceeded");

    // CRITICAL FINANCIAL INVARIANT: The payment hash must NOT be marked consumed!
    let payment_auth = ctx
        .db
        .payment_store()
        .get_authorization_state(&satsgrid::common::types::PaymentHash(pay_hash))
        .await
        .unwrap();
    assert!(
        payment_auth.is_none(),
        "Rejected request must not consume or write payment authorization"
    );
}

#[tokio::test]
async fn test_session_saturation_rejects_before_budget_reservation() {
    let ctx = setup_test_context(ConcurrencyConfig {
        max_concurrent_requests: 5,
        max_concurrent_per_session: 1, // 1 per session
        admission_timeout_secs: 0,
    })
    .await;

    let (session_id, auth_header) = fund_session(&ctx, 100).await;

    // Manually occupy the 1 session permit
    let _occupying_permit = ctx
        .concurrency_manager
        .try_acquire_all_now(Some(&session_id))
        .unwrap();

    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": false
    });

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, &auth_header)
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp = ctx.app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(resp.headers()["retry-after"], "1");

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json_resp: Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(json_resp["error"]["type"], "concurrency_limit_error");
    assert_eq!(json_resp["error"]["code"], "session_concurrency_exceeded");

    // CRITICAL FINANCIAL INVARIANT: Budget remains 100 available, 0 reserved, 0 spent
    let session = ctx
        .db
        .session_store()
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session.remaining_sats, Satoshis(100));
    assert_eq!(session.reserved_sats, Satoshis(0));
    assert_eq!(session.spent_sats, Satoshis(0));
}

#[tokio::test]
async fn test_different_sessions_remain_isolated_under_concurrency_limits() {
    let ctx = setup_test_context(ConcurrencyConfig {
        max_concurrent_requests: 5,
        max_concurrent_per_session: 1,
        admission_timeout_secs: 0,
    })
    .await;

    let (session_a, auth_a) = fund_session(&ctx, 100).await;
    let (_session_b, auth_b) = fund_session(&ctx, 100).await;

    // Occupy Session A's permit
    let _occupying_a = ctx
        .concurrency_manager
        .try_acquire_all_now(Some(&session_a))
        .unwrap();

    // Mock upstream response for Session B
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chat-test",
            "choices": [{"message": {"role": "assistant", "content": "hello"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 20, "total_tokens": 30}
        })))
        .mount(&ctx.mock_server)
        .await;

    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": false
    });

    // Request from Session B must succeed even while Session A is fully saturated!
    let req_b = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, &auth_b)
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp_b = ctx.app.clone().oneshot(req_b).await.unwrap();
    assert_eq!(resp_b.status(), StatusCode::OK);

    // Request from Session A must be rejected
    let req_a = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, &auth_a)
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp_a = ctx.app.oneshot(req_a).await.unwrap();
    assert_eq!(resp_a.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn test_successful_non_streaming_request_releases_permit() {
    let ctx = setup_test_context(ConcurrencyConfig {
        max_concurrent_requests: 1, // Only 1 global slot
        max_concurrent_per_session: 1,
        admission_timeout_secs: 0,
    })
    .await;

    let (_session_id, auth_header) = fund_session(&ctx, 100).await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chat-test",
            "choices": [{"message": {"role": "assistant", "content": "hello"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 20, "total_tokens": 30}
        })))
        .mount(&ctx.mock_server)
        .await;

    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": false
    });

    let req1 = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, &auth_header)
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp1 = ctx.app.clone().oneshot(req1).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);

    // After completion, global and session permits must be freed!
    assert_eq!(ctx.concurrency_manager.global_available_permits(), 1);

    // Second request must succeed immediately because permit was released
    let req2 = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, &auth_header)
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp2 = ctx.app.oneshot(req2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_upstream_failure_releases_permit() {
    let ctx = setup_test_context(ConcurrencyConfig {
        max_concurrent_requests: 1,
        max_concurrent_per_session: 1,
        admission_timeout_secs: 0,
    })
    .await;

    let (_session_id, auth_header) = fund_session(&ctx, 100).await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal upstream error"))
        .mount(&ctx.mock_server)
        .await;

    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": false
    });

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, &auth_header)
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp = ctx.app.oneshot(req).await.unwrap();
    // Upstream 500 error is mapped to Bad Gateway (502)
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);

    // Permit must be released back to the manager
    assert_eq!(ctx.concurrency_manager.global_available_permits(), 1);
}

#[tokio::test]
async fn test_streaming_holds_permit_while_active_and_releases_on_done() {
    let ctx = setup_test_context(ConcurrencyConfig {
        max_concurrent_requests: 1, // Only 1 global slot
        max_concurrent_per_session: 1,
        admission_timeout_secs: 0,
    })
    .await;

    let (_session_id, auth_header) = fund_session(&ctx, 100).await;

    // Upstream mocks an SSE stream with a short pause between chunks
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\ndata: [DONE]\n\n",
                ),
        )
        .mount(&ctx.mock_server)
        .await;

    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": true
    });

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, &auth_header)
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp = ctx.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "text/event-stream");

    // While stream is actively being read, permit is held in MonitoredStream.
    // Read body to completion:
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);
    assert!(body_str.contains("[DONE]"));

    // After natural completion, permit must be freed!
    assert_eq!(ctx.concurrency_manager.global_available_permits(), 1);
}

#[tokio::test]
async fn test_streaming_releases_permit_on_early_client_disconnect() {
    let ctx = setup_test_context(ConcurrencyConfig {
        max_concurrent_requests: 1,
        max_concurrent_per_session: 1,
        admission_timeout_secs: 0,
    })
    .await;

    let (_session_id, auth_header) = fund_session(&ctx, 100).await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("data: {\"choices\":[{\"delta\":{\"content\":\"chunk1\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"chunk2\"}}]}\n\n"),
        )
        .mount(&ctx.mock_server)
        .await;

    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": true
    });

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, &auth_header)
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp = ctx.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Simulate early disconnect by dropping the response body without reading to end
    drop(resp);

    // Give background tasks a brief moment to finish drop execution
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Concurrency permit must be released back to the manager
    assert_eq!(
        ctx.concurrency_manager.global_available_permits(),
        1,
        "Permit must be released when client disconnects early"
    );
}

#[tokio::test]
async fn test_streaming_active_blocks_concurrent_request() {
    let ctx = setup_test_context(ConcurrencyConfig {
        max_concurrent_requests: 1, // Only 1 global slot
        max_concurrent_per_session: 1,
        admission_timeout_secs: 0,
    })
    .await;

    let (_session_id, auth_header) = fund_session(&ctx, 100).await;

    // Use a mock TCP listener to hold the stream open
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            use tokio::io::AsyncWriteExt;
            let resp = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"start\"}}]}\n\n";
            let _ = socket.write_all(resp.as_bytes()).await;
            // Sleep to keep socket open and simulate mid-flight stream
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let done = "data: [DONE]\n\n";
            let _ = socket.write_all(done.as_bytes()).await;
        }
    });

    // Point upstream to local mock listener
    let proxy = OpenAiProxy::new(format!("http://{}", addr));

    let test_cfg = NodeConfig {
        upstream: UpstreamConfig {
            url: format!("http://{}", addr),
            connect_timeout_secs: 2,
            response_header_timeout_secs: 5,
            stream_idle_timeout_secs: 5,
            total_request_timeout_secs: 10,
        },
        pricing: PricingConfig::new(Satoshis(10)),
        concurrency: ConcurrencyConfig {
            max_concurrent_requests: 1,
            max_concurrent_per_session: 1,
            admission_timeout_secs: 0,
        },
        data_dir: ctx._dir.path().to_string_lossy().to_string(),
        ..Default::default()
    };

    let app_state = AppState {
        config: Arc::new(test_cfg),
        live_pricing: Arc::new(tokio::sync::RwLock::new(PricingConfig::new(Satoshis(10)))),
        lightning: ctx.lightning.clone(),
        budget_manager: ctx.budget_manager.clone(),
        macaroon_service: ctx.macaroon_service.clone(),
        proxy,
        stats: Arc::new(satsgrid::node::api::NodeStats::default()),
        admin_token: Arc::new("test-admin-token".to_string()),
        db: Some(ctx.db.clone()),
        concurrency_manager: ctx.concurrency_manager.clone(),
    };
    let state = create_routes(app_state);

    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": true
    });

    let req1 = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, &auth_header)
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp1 = state.clone().oneshot(req1).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);

    // While stream 1 is active (holding the body stream open):
    let req2 = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, &auth_header)
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp2 = state.oneshot(req2).await.unwrap();
    // Capacity = 1; must be rejected because stream 1 retains the permit!
    assert_eq!(resp2.status(), StatusCode::SERVICE_UNAVAILABLE);

    // Consume stream 1 to finish
    let _ = axum::body::to_bytes(resp1.into_body(), usize::MAX).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_eq!(ctx.concurrency_manager.global_available_permits(), 1);
}

#[tokio::test]
async fn test_completed_idempotency_replay_rejected_without_consuming_concurrency_slot() {
    let ctx = setup_test_context(ConcurrencyConfig {
        max_concurrent_requests: 1,
        max_concurrent_per_session: 1,
        admission_timeout_secs: 0,
    })
    .await;

    let (_session_id, auth_header) = fund_session(&ctx, 100).await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chat-test",
            "choices": [{"message": {"role": "assistant", "content": "hello"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 20, "total_tokens": 30}
        })))
        .mount(&ctx.mock_server)
        .await;

    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": false
    });

    let idempotency_key = "test-idem-replay-key-123";

    // Request 1 completes with idempotency key
    let req1 = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, &auth_header)
        .header("Idempotency-Key", idempotency_key)
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp1 = ctx.app.clone().oneshot(req1).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);

    // Now occupy the global permit to verify whether replay check happens BEFORE concurrency acquisition
    let _occupying_permit = ctx.concurrency_manager.try_acquire_all_now(None).unwrap();
    assert_eq!(ctx.concurrency_manager.global_available_permits(), 0);

    // Replay of completed request must be rejected with 401/VerificationFailed from idempotency check,
    // NOT 503 from concurrency saturation!
    let req2 = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, &auth_header)
        .header("Idempotency-Key", idempotency_key)
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp2 = ctx.app.oneshot(req2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::UNAUTHORIZED); // VerificationFailed maps to 401
    let body_bytes = axum::body::to_bytes(resp2.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);
    assert!(body_str.contains("Idempotency key has already been consumed for this session"));
}

#[tokio::test]
async fn test_simultaneous_idempotency_race_rolls_back_losing_reservation() {
    let ctx = setup_test_context(ConcurrencyConfig {
        max_concurrent_requests: 5,
        max_concurrent_per_session: 5,
        admission_timeout_secs: 0,
    })
    .await;

    let (session_id, auth_header) = fund_session(&ctx, 100).await;
    let idem_key = "simultaneous_http_barrier_idem_key_42";

    // Mock upstream response with a small delay so both requests race past admission into reservation
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_millis(100))
                .set_body_json(json!({
                    "id": "chat-simul-1",
                    "choices": [{"message": {"role": "assistant", "content": "mocked response"}}],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 20, "total_tokens": 30}
                })),
        )
        .mount(&ctx.mock_server)
        .await;

    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": false
    });

    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let mut handles = Vec::new();

    for _ in 0..2 {
        let app = ctx.app.clone();
        let auth = auth_header.clone();
        let body_bytes = serde_json::to_vec(&payload).unwrap();
        let b = barrier.clone();

        handles.push(tokio::spawn(async move {
            b.wait().await;
            let req = Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, auth)
                .header("Idempotency-Key", idem_key)
                .body(Body::from(body_bytes))
                .unwrap();

            app.oneshot(req).await.unwrap()
        }));
    }

    let mut responses = Vec::new();
    for h in handles {
        let resp = h.await.unwrap();
        let status = resp.status();
        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8_lossy(&body_bytes).to_string();
        responses.push((status, body_str));
    }

    // 1. Exactly one request must succeed (200 OK)
    let ok_count = responses
        .iter()
        .filter(|(s, _)| *s == StatusCode::OK)
        .count();
    assert_eq!(ok_count, 1, "Exactly one request must succeed");

    // 2. The racing duplicate must receive 401 Unauthorized (VerificationFailed)
    let conflict_count = responses
        .iter()
        .filter(|(s, _)| *s == StatusCode::UNAUTHORIZED)
        .count();
    assert_eq!(
        conflict_count, 1,
        "Duplicate concurrent request must be rejected with 401 (VerificationFailed)"
    );

    // Verify error message in the losing response
    let conflict_resp = responses
        .iter()
        .find(|(s, _)| *s == StatusCode::UNAUTHORIZED)
        .unwrap();
    assert!(
        conflict_resp
            .1
            .contains("A request with this idempotency key is currently in flight"),
        "Expected in-flight conflict message, got: {}",
        conflict_resp.1
    );

    // 3. Database invariants: Exactly ONE inference_request was created
    let record = ctx
        .db
        .request_store()
        .find_by_idempotency_key(&session_id, idem_key)
        .await
        .unwrap();
    assert!(record.is_some(), "Successful request record must exist");

    // 4. Session budget & accounting invariant:
    // Remaining (90) + Reserved (0) + Spent (10) == Total (100)
    // The losing request MUST NOT have leaked a reservation!
    let session = ctx
        .db
        .session_store()
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        session.remaining_sats,
        Satoshis(90),
        "Remaining budget must be 90"
    );
    assert_eq!(
        session.reserved_sats,
        Satoshis(0),
        "No orphaned reserved balance"
    );
    assert_eq!(
        session.spent_sats,
        Satoshis(10),
        "Spent budget must be exactly 10"
    );
    assert_eq!(
        session.remaining_sats.0 + session.reserved_sats.0 + session.spent_sats.0,
        session.total_budget_sats.0,
        "Session balance invariant: remaining + reserved + spent == total"
    );

    // 5. Check reservations table: exactly one finalized, and one released (or only one created)
    let res_list = ctx
        .db
        .session_store()
        .get_accounting_entries(&session_id)
        .await
        .unwrap();
    // Funding (100) + reservation(s) + debit(10) + optional release(10)
    let total_debit: i64 = res_list
        .iter()
        .filter(|row| row.1 == "session_debit")
        .map(|row| row.2)
        .sum();
    assert_eq!(total_debit, 10, "Only 10 sats debited in ledger");

    // 6. Concurrency permits: all 5 global permits must be returned
    assert_eq!(ctx.concurrency_manager.global_available_permits(), 5);
}
