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
use std::time::Duration;
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
}

async fn setup_test_context(upstream_config: UpstreamConfig) -> TestContext {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));
    let mock_server = MockServer::start().await;

    let mut final_upstream = upstream_config;
    final_upstream.url = mock_server.uri();

    let config = NodeConfig {
        server: ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            admin_token: Some("test-admin-token".to_string()),
            ..Default::default()
        },
        pricing: PricingConfig::new(Satoshis(10)),
        upstream: final_upstream.clone(),
        data_dir: dir.path().to_string_lossy().to_string(),
        ..Default::default()
    };

    let lightning = Arc::new(MockLightningBackend::new());
    let macaroon_service = Arc::new(MacaroonService::new([7u8; 32].to_vec(), "satsgrid-test"));
    let budget_manager = Arc::new(SessionBudgetManager::with_store(db.session_store()));
    let proxy = OpenAiProxy::new_with_timeouts(
        config.upstream.url.clone(),
        Duration::from_secs(config.upstream.connect_timeout_secs),
        Duration::from_secs(config.upstream.response_header_timeout_secs),
    );

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
        mock_server,
    }
}

#[tokio::test]
async fn test_upstream_response_header_timeout_non_streaming() {
    let upstream_cfg = UpstreamConfig {
        url: "".to_string(),
        connect_timeout_secs: 1,
        response_header_timeout_secs: 1, // 1 sec header timeout
        stream_idle_timeout_secs: 5,
        total_request_timeout_secs: 5,
    };
    let ctx = setup_test_context(upstream_cfg).await;

    // Upstream mocks 2.5 second delay (> 1s header timeout)
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(2500))
                .set_body_json(json!({"id": "delayed", "choices": []})),
        )
        .mount(&ctx.mock_server)
        .await;

    // Fund a session
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
    assert_eq!(resp.status(), StatusCode::GATEWAY_TIMEOUT);

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let err_json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(err_json["error"]["type"], "upstream_timeout_error");

    // Wait a brief moment for background guard cleanup
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify session reservation is marked reconciliation_required (upstream execution was ambiguous)
    let session = ctx
        .db
        .session_store()
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session.remaining_sats, Satoshis(90)); // 10 sats remain reserved/un-refunded
    assert_eq!(session.reserved_sats, Satoshis(10));

    let res_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM session_reservations WHERE session_id = ?1 AND status = 'reconciliation_required'",
    )
    .bind(session_id.0.to_string())
    .fetch_one(ctx.db.pool())
    .await
    .unwrap();
    assert_eq!(res_count, 1);
}

#[tokio::test]
async fn test_upstream_connect_failure_releases_session_reservation() {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));

    // Point to a non-listening local port to trigger immediate connection refused
    let config = NodeConfig {
        server: ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            admin_token: Some("test-admin-token".to_string()),
            ..Default::default()
        },
        pricing: PricingConfig::new(Satoshis(10)),
        upstream: UpstreamConfig {
            url: "http://127.0.0.1:45123".to_string(), // Closed port
            connect_timeout_secs: 1,
            response_header_timeout_secs: 2,
            stream_idle_timeout_secs: 5,
            total_request_timeout_secs: 5,
        },
        data_dir: dir.path().to_string_lossy().to_string(),
        ..Default::default()
    };

    let lightning = Arc::new(MockLightningBackend::new());
    let macaroon_service = Arc::new(MacaroonService::new([7u8; 32].to_vec(), "satsgrid-test"));
    let budget_manager = Arc::new(SessionBudgetManager::with_store(db.session_store()));
    let proxy = OpenAiProxy::new_with_timeouts(
        config.upstream.url.clone(),
        Duration::from_secs(config.upstream.connect_timeout_secs),
        Duration::from_secs(config.upstream.response_header_timeout_secs),
    );

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

    // Fund session
    let session_uuid = uuid::Uuid::new_v4();
    let session_id = SessionId(session_uuid);
    let invoice = lightning
        .create_invoice(Satoshis(100), "Session")
        .await
        .unwrap();
    let preimage = lightning.pay_invoice(&invoice.bolt11).await.unwrap();
    budget_manager
        .open_session_with_details(
            &session_id,
            Some(&invoice.payment_hash),
            Satoshis(100),
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .unwrap();

    let macaroon = macaroon_service
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

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);

    // Wait briefly for drop guard
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Definite connect error -> session reservation safely released, restoring budget to 100 sats!
    let session = db
        .session_store()
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session.remaining_sats, Satoshis(100));
    assert_eq!(session.reserved_sats, Satoshis(0));

    let res_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM session_reservations WHERE session_id = ?1 AND status = 'reserved'",
    )
    .bind(session_id.0.to_string())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        res_count, 0,
        "Reservation must be released on definite connect failure"
    );
}

#[tokio::test]
async fn test_sse_streaming_idle_timeout_emits_error_and_no_done() {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));

    // We create a mock TCP server that sends headers and 1 chunk, then sleeps without closing
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;

            let response_headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";
            let _ = socket.write_all(response_headers.as_bytes()).await;

            // Chunk: "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n"
            let chunk_data = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n";
            let chunk_header = format!("{:X}\r\n{}\r\n", chunk_data.len(), chunk_data);
            let _ = socket.write_all(chunk_header.as_bytes()).await;
            let _ = socket.flush().await;

            // Now pause for 3 seconds without sending anything or closing (idle timeout is 1 sec)
            tokio::time::sleep(Duration::from_millis(3000)).await;
        }
    });

    let config = NodeConfig {
        server: ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            admin_token: Some("test-admin-token".to_string()),
            ..Default::default()
        },
        pricing: PricingConfig::new(Satoshis(10)),
        upstream: UpstreamConfig {
            url: format!("http://{}", local_addr),
            connect_timeout_secs: 2,
            response_header_timeout_secs: 2,
            stream_idle_timeout_secs: 1, // 1 sec idle timeout
            total_request_timeout_secs: 10,
        },
        data_dir: dir.path().to_string_lossy().to_string(),
        ..Default::default()
    };

    let lightning = Arc::new(MockLightningBackend::new());
    let macaroon_service = Arc::new(MacaroonService::new([7u8; 32].to_vec(), "satsgrid-test"));
    let budget_manager = Arc::new(SessionBudgetManager::with_store(db.session_store()));
    let proxy = OpenAiProxy::new_with_timeouts(
        config.upstream.url.clone(),
        Duration::from_secs(config.upstream.connect_timeout_secs),
        Duration::from_secs(config.upstream.response_header_timeout_secs),
    );

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

    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": true
    });

    // Pay-per-request invoice
    let invoice = lightning
        .create_invoice(Satoshis(10), "Test")
        .await
        .unwrap();
    let preimage = lightning.pay_invoice(&invoice.bolt11).await.unwrap();
    let macaroon = macaroon_service
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

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, &auth_header)
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "text/event-stream");

    // Read stream events
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);

    // Must contain first chunk
    assert!(body_str.contains("Hello"));

    // Must contain error event from idle timeout
    assert!(body_str.contains("event: error"));
    assert!(body_str.contains("stream_idle_timeout"));

    // CRITICAL INVARIANT: Must NOT contain [DONE]!
    assert!(
        !body_str.contains("[DONE]"),
        "Stalled stream must never emit [DONE]"
    );

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify database status
    let req_record = db
        .request_store()
        .get_request_by_payment_hash(&invoice.payment_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        req_record.status,
        satsgrid::node::storage::RequestStatus::StreamInterrupted
    );
    assert_eq!(
        req_record.error_code,
        Some("stream_idle_timeout".to_string())
    );
}

#[tokio::test]
async fn test_atomic_non_streaming_finalization_consistency() {
    let ctx = setup_test_context(UpstreamConfig::default()).await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "cmpl_1",
            "choices": [{"message": {"role": "assistant", "content": "Success"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 20, "total_tokens": 30}
        })))
        .mount(&ctx.mock_server)
        .await;

    // Fund a session
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
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify atomic consistency in DB: session reservation is finalized, session spent is 10, and request is completed
    let session = ctx
        .db
        .session_store()
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session.remaining_sats, Satoshis(90));
    assert_eq!(session.reserved_sats, Satoshis(0));
    assert_eq!(session.spent_sats, Satoshis(10));

    let res_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM session_reservations WHERE session_id = ?1 AND status = 'reserved'",
    )
    .bind(session_id.0.to_string())
    .fetch_one(ctx.db.pool())
    .await
    .unwrap();
    assert_eq!(
        res_count, 0,
        "No active reserved reservations should remain"
    );
}

#[tokio::test]
async fn test_non_streaming_total_deadline_timeout() {
    let upstream_cfg = UpstreamConfig {
        url: "".to_string(),
        connect_timeout_secs: 1,
        response_header_timeout_secs: 3,
        stream_idle_timeout_secs: 5,
        total_request_timeout_secs: 1, // 1 sec total deadline
    };
    let ctx = setup_test_context(upstream_cfg).await;

    // Upstream takes 2.0 seconds (> 1 sec total deadline)
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(2000))
                .set_body_json(json!({"id": "slow", "choices": []})),
        )
        .mount(&ctx.mock_server)
        .await;

    // Pay-per-request invoice
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
    assert_eq!(resp.status(), StatusCode::GATEWAY_TIMEOUT);

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let err_json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(err_json["error"]["type"], "upstream_timeout_error");
}

#[tokio::test]
async fn test_streaming_total_deadline_terminates_active_chunks() {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));

    // We create a mock TCP server that streams chunks continuously every 200ms
    // (well below the 2-second idle timeout), but the total stream lasts 2.5 seconds
    // (exceeding the 1-second total request deadline).
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;

            let response_headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";
            let _ = socket.write_all(response_headers.as_bytes()).await;

            // Send 10 chunks spaced by 250ms (total ~2.5s)
            for i in 0..10 {
                let chunk_data = format!(
                    "data: {{\"choices\":[{{\"delta\":{{\"content\":\"token_{} \"}}}}]}}\n\n",
                    i
                );
                let chunk_header = format!("{:X}\r\n{}\r\n", chunk_data.len(), chunk_data);
                let _ = socket.write_all(chunk_header.as_bytes()).await;
                let _ = socket.flush().await;
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    });

    let config = NodeConfig {
        server: ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            admin_token: Some("test-admin-token".to_string()),
            ..Default::default()
        },
        pricing: PricingConfig::new(Satoshis(10)),
        upstream: UpstreamConfig {
            url: format!("http://{}", local_addr),
            connect_timeout_secs: 2,
            response_header_timeout_secs: 2,
            stream_idle_timeout_secs: 3, // 3 sec idle timeout (never expires between 250ms chunks)
            total_request_timeout_secs: 1, // 1 sec total deadline (must fire!)
        },
        data_dir: dir.path().to_string_lossy().to_string(),
        ..Default::default()
    };

    let lightning = Arc::new(MockLightningBackend::new());
    let macaroon_service = Arc::new(MacaroonService::new([7u8; 32].to_vec(), "satsgrid-test"));
    let budget_manager = Arc::new(SessionBudgetManager::with_store(db.session_store()));
    let proxy = OpenAiProxy::new_with_timeouts(
        config.upstream.url.clone(),
        Duration::from_secs(config.upstream.connect_timeout_secs),
        Duration::from_secs(config.upstream.response_header_timeout_secs),
    );

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

    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": true
    });

    // Pay-per-request invoice
    let invoice = lightning
        .create_invoice(Satoshis(10), "Test")
        .await
        .unwrap();
    let preimage = lightning.pay_invoice(&invoice.bolt11).await.unwrap();
    let macaroon = macaroon_service
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

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, &auth_header)
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "text/event-stream");

    // Read stream events
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);

    // Must contain initial tokens
    assert!(body_str.contains("token_0"));

    // Must contain error event from total deadline timeout
    assert!(body_str.contains("event: error"));
    assert!(body_str.contains("stream_total_timeout"));

    // CRITICAL: Must NOT contain [DONE]
    assert!(
        !body_str.contains("[DONE]"),
        "Timed out stream must not emit [DONE]"
    );

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify database record status
    let req_record = db
        .request_store()
        .get_request_by_payment_hash(&invoice.payment_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        req_record.status,
        satsgrid::node::storage::RequestStatus::StreamInterrupted
    );
    assert_eq!(
        req_record.error_code,
        Some("stream_total_timeout".to_string())
    );
}
