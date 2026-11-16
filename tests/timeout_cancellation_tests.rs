use axum::{
    body::Body,
    http::{header, Request, StatusCode},
};
use satsgrid::common::types::{Satoshis, SessionId};
use satsgrid::config::schema::{
    LightningConfig, NodeConfig, PricingConfig, ServerConfig, UpstreamConfig,
};
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
    stats: Arc<satsgrid::node::api::NodeStats>,
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
        lightning: LightningConfig {
            allow_mock: true,
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
    let stats = Arc::new(satsgrid::node::api::NodeStats::default());
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
        stats: stats.clone(),
        admin_token: Arc::new("test-admin-token".to_string()),
        db: Some(db.clone()),
        concurrency_manager: Arc::new(satsgrid::node::concurrency::ConcurrencyManager::new(
            config.concurrency.max_concurrent_requests,
            config.concurrency.max_concurrent_per_session,
            config.concurrency.admission_timeout_secs,
        )),
    };

    let app = create_routes(state);

    TestContext {
        app,
        db,
        lightning,
        macaroon_service,
        budget_manager,
        stats,
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
        lightning: LightningConfig {
            allow_mock: true,
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
        concurrency_manager: Arc::new(satsgrid::node::concurrency::ConcurrencyManager::new(
            config.concurrency.max_concurrent_requests,
            config.concurrency.max_concurrent_per_session,
            config.concurrency.admission_timeout_secs,
        )),
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

    let reconciliation_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reconciliation_items WHERE item_type = 'inference_request'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(reconciliation_count, 0);
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
        lightning: LightningConfig {
            allow_mock: true,
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
        concurrency_manager: Arc::new(satsgrid::node::concurrency::ConcurrencyManager::new(
            config.concurrency.max_concurrent_requests,
            config.concurrency.max_concurrent_per_session,
            config.concurrency.admission_timeout_secs,
        )),
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

    let reconciliation_items = db
        .reconciliation_store()
        .list_items(None, 10)
        .await
        .unwrap();
    assert_eq!(reconciliation_items.len(), 1);
    assert_eq!(
        reconciliation_items[0].item_type,
        satsgrid::node::storage::ReconciliationItemType::InferenceRequest
    );
    assert_eq!(reconciliation_items[0].reference_id, req_record.id);
    assert_eq!(reconciliation_items[0].reason, "stream_idle_timeout");
    assert_eq!(
        reconciliation_items[0].status,
        satsgrid::node::storage::ReconciliationStatus::Unresolved
    );
    assert!(db
        .receipt_store()
        .get_receipt_by_request_id(&req_record.id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn test_successful_stream_persists_one_verifiable_signed_receipt() {
    use satsgrid::node::receipt::{ReceiptSigner, ED25519_ALGORITHM_NAME};

    let ctx = setup_test_context(UpstreamConfig::default()).await;
    let signer = Arc::new(
        ReceiptSigner::load_or_generate(ctx._dir.path(), "satsgrid-node").expect("signer"),
    );
    AppState::set_receipt_signer(signer.clone());

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\ndata: [DONE]\n\n",
                ),
        )
        .mount(&ctx.mock_server)
        .await;

    let session_uuid = uuid::Uuid::new_v4();
    let session_id = SessionId(session_uuid);
    let invoice = ctx
        .lightning
        .create_invoice(Satoshis(100), "Streaming receipt test")
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
                Caveat::ExpiresAt((chrono::Utc::now().timestamp() + 3600) as u64),
                Caveat::Model("llama3.2".to_string()),
            ],
        )
        .unwrap();
    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": true
    });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::AUTHORIZATION,
            format!("L402 {}:{}", macaroon.to_base64().unwrap(), preimage),
        )
        .body(Body::from(serde_json::to_vec(&payload).unwrap()))
        .unwrap();

    let resp = ctx.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8_lossy(&body);
    assert!(body_str.contains("[DONE]"));
    assert!(body_str.contains("event: receipt"));
    assert!(body_str.contains("receipt_id"));

    let request = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let request_id: Option<String> = sqlx::query_scalar(
                "SELECT id FROM inference_requests WHERE session_id = ?1 ORDER BY created_at DESC LIMIT 1",
            )
            .bind(session_id.0.to_string())
            .fetch_optional(ctx.db.pool())
            .await
            .unwrap();
            let request_id = request_id.expect("stream inference request");
            let request = ctx
                .db
                .request_store()
                .get_request(&request_id)
                .await
                .unwrap()
                .unwrap();
            if request.status == satsgrid::node::storage::RequestStatus::Completed {
                break request;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("stream completion finalization");
    assert_eq!(
        request.status,
        satsgrid::node::storage::RequestStatus::Completed
    );
    let finalized_session = ctx
        .db
        .session_store()
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(finalized_session.reserved_sats, Satoshis(0));
    assert_eq!(finalized_session.spent_sats, Satoshis(10));
    assert_eq!(
        ctx.stats
            .total_requests
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(
        ctx.stats
            .total_sats_earned
            .load(std::sync::atomic::Ordering::SeqCst),
        10
    );

    let receipt = ctx
        .db
        .receipt_store()
        .get_receipt_by_request_id(&request.id)
        .await
        .unwrap()
        .expect("stream completion receipt");
    assert_eq!(receipt.payload.request_id, request.id);
    assert_eq!(receipt.payload.inference_status, "completed");
    assert_eq!(receipt.algorithm, ED25519_ALGORITHM_NAME);
    ReceiptSigner::verify_signature(&receipt, None).expect("verify streaming receipt signature");

    let body_str = String::from_utf8_lossy(&body);
    assert!(body_str.contains("event: receipt"));
    assert!(body_str.contains(&format!(
        "\"receipt_id\":\"{}\"",
        receipt.payload.receipt_id
    )));
    assert!(body_str.contains("data: [DONE]"));

    // Replaying the successful completion callback must remain idempotent.
    ctx.db
        .complete_streaming_request_with_receipt(
            &request.id,
            request.reservation_id.as_deref(),
            receipt.payload.prompt_tokens,
            receipt.payload.completion_tokens,
            &receipt,
        )
        .await
        .expect("repeated completion is idempotent");
    let receipt_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM receipts WHERE request_id = ?1")
            .bind(&request.id)
            .fetch_one(ctx.db.pool())
            .await
            .unwrap();
    assert_eq!(receipt_count, 1);
    assert_eq!(
        ctx.stats
            .total_requests
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(
        ctx.stats
            .total_sats_earned
            .load(std::sync::atomic::Ordering::SeqCst),
        10
    );
}

#[tokio::test]
async fn test_stream_eof_without_done_marker_does_not_create_success_receipt() {
    use satsgrid::node::receipt::ReceiptSigner;

    let ctx = setup_test_context(UpstreamConfig::default()).await;
    let signer = Arc::new(
        ReceiptSigner::load_or_generate(ctx._dir.path(), "satsgrid-node").expect("signer"),
    );
    AppState::set_receipt_signer(signer);
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n"),
        )
        .mount(&ctx.mock_server)
        .await;

    let invoice = ctx
        .lightning
        .create_invoice(Satoshis(10), "Truncated streaming receipt test")
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
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::AUTHORIZATION,
            format!("L402 {}:{}", macaroon.to_base64().unwrap(), preimage),
        )
        .body(Body::from(
            json!({
                "model": "llama3.2",
                "messages": [{"role": "user", "content": "hi"}],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let resp = ctx.app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();

    let request = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(request) = ctx
                .db
                .request_store()
                .get_request_by_payment_hash(&invoice.payment_hash)
                .await
                .unwrap()
            {
                if request.status != satsgrid::node::storage::RequestStatus::Processing {
                    break request;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("truncated stream interruption state");
    assert_eq!(
        request.status,
        satsgrid::node::storage::RequestStatus::StreamInterrupted
    );
    assert!(ctx
        .db
        .receipt_store()
        .get_receipt_by_request_id(&request.id)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        ctx.stats
            .total_requests
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(
        ctx.stats
            .total_sats_earned
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn test_stream_receipt_persistence_failure_is_durably_recoverable() {
    use satsgrid::node::receipt::ReceiptSigner;

    let ctx = setup_test_context(UpstreamConfig::default()).await;
    let signer = Arc::new(
        ReceiptSigner::load_or_generate(ctx._dir.path(), "satsgrid-node").expect("signer"),
    );
    AppState::set_receipt_signer(signer);
    sqlx::query(
        "CREATE TRIGGER reject_stream_receipt BEFORE INSERT ON receipts BEGIN SELECT RAISE(FAIL, 'injected receipt persistence failure'); END",
    )
    .execute(ctx.db.pool())
    .await
    .unwrap();

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\ndata: [DONE]\n\n",
                ),
        )
        .mount(&ctx.mock_server)
        .await;

    let session_uuid = uuid::Uuid::new_v4();
    let session_id = SessionId(session_uuid);
    let invoice = ctx
        .lightning
        .create_invoice(Satoshis(100), "Receipt persistence failure test")
        .await
        .unwrap();
    ctx.budget_manager
        .open_session_with_details(
            &session_id,
            Some(&invoice.payment_hash),
            Satoshis(100),
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .unwrap();
    let preimage = ctx.lightning.pay_invoice(&invoice.bolt11).await.unwrap();
    let macaroon = ctx
        .macaroon_service
        .mint(
            &invoice.payment_hash,
            vec![
                Caveat::Session(session_uuid.to_string()),
                Caveat::Budget(100),
                Caveat::Capability("inference".to_string()),
                Caveat::ExpiresAt((chrono::Utc::now().timestamp() + 3600) as u64),
                Caveat::Model("llama3.2".to_string()),
            ],
        )
        .unwrap();
    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::AUTHORIZATION,
            format!("L402 {}:{}", macaroon.to_base64().unwrap(), preimage),
        )
        .body(Body::from(
            json!({
                "model": "llama3.2",
                "messages": [{"role": "user", "content": "receipt failure"}],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();
    let response = ctx.app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8_lossy(&body);
    assert!(body_str.contains("[DONE]"));
    assert!(!body_str.contains("event: receipt"));
    assert!(!body_str.contains("receipt_id"));

    let request = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let request_id: Option<String> = sqlx::query_scalar(
                "SELECT id FROM inference_requests WHERE session_id = ?1 ORDER BY created_at DESC LIMIT 1",
            )
            .bind(session_id.0.to_string())
            .fetch_optional(ctx.db.pool())
            .await
            .unwrap();
            if let Some(request_id) = request_id {
                let request = ctx
                    .db
                    .request_store()
                    .get_request(&request_id)
                    .await
                    .unwrap()
                    .expect("stream request");
                if request.status != satsgrid::node::storage::RequestStatus::Processing {
                    break request;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("finalization recovery state");
    assert_eq!(
        request.status,
        satsgrid::node::storage::RequestStatus::ReconciliationRequired
    );
    assert_eq!(
        request.error_code.as_deref(),
        Some("stream_finalization_failed")
    );
    assert!(ctx
        .db
        .receipt_store()
        .get_receipt_by_request_id(&request.id)
        .await
        .unwrap()
        .is_none());

    let item = ctx
        .db
        .reconciliation_store()
        .list_items(
            Some(satsgrid::node::storage::ReconciliationStatus::Unresolved),
            10,
        )
        .await
        .unwrap()
        .into_iter()
        .find(|item| item.reference_id == request.id)
        .expect("durable reconciliation item");
    assert_eq!(item.reason, "stream_finalization_failed");
    assert!(item.payment_hash.is_none());

    let reservation_status: String =
        sqlx::query_scalar("SELECT status FROM session_reservations WHERE id = ?1")
            .bind(request.reservation_id.as_deref().unwrap())
            .fetch_one(ctx.db.pool())
            .await
            .unwrap();
    assert_eq!(reservation_status, "reconciliation_required");
    let session = ctx
        .db
        .session_store()
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session.reserved_sats, Satoshis(10));
    assert_eq!(session.spent_sats, Satoshis(0));
    assert_eq!(
        ctx.stats
            .total_requests
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(
        ctx.stats
            .total_sats_earned
            .load(std::sync::atomic::Ordering::SeqCst),
        0
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
async fn test_successful_non_streaming_persists_receipt_and_returns_header() {
    use satsgrid::node::receipt::{ReceiptSigner, ED25519_ALGORITHM_NAME};

    let ctx = setup_test_context(UpstreamConfig::default()).await;
    let signer = Arc::new(
        ReceiptSigner::load_or_generate(ctx._dir.path(), "satsgrid-node").expect("signer"),
    );
    AppState::set_receipt_signer(signer.clone());

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "cmpl_receipt_success",
            "choices": [{"message": {"role": "assistant", "content": "Receipt success"}}],
            "usage": {"prompt_tokens": 12, "completion_tokens": 24, "total_tokens": 36}
        })))
        .mount(&ctx.mock_server)
        .await;

    let session_uuid = uuid::Uuid::new_v4();
    let session_id = SessionId(session_uuid);
    let invoice = ctx
        .lightning
        .create_invoice(Satoshis(100), "Non-streaming receipt test")
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
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify response header contains receipt ID
    let receipt_id_header = resp
        .headers()
        .get("x-satsgrid-receipt-id")
        .expect("x-satsgrid-receipt-id header must be present on success");
    let receipt_id_str = receipt_id_header.to_str().unwrap().to_string();
    assert!(receipt_id_str.starts_with("rcpt_"));

    // Verify receipt retrieval returns the persisted receipt
    let receipt = ctx
        .db
        .receipt_store()
        .get_receipt(&receipt_id_str)
        .await
        .unwrap()
        .expect("receipt must be persisted in database");
    assert_eq!(receipt.payload.receipt_id, receipt_id_str);
    assert_eq!(receipt.payload.inference_status, "completed");
    assert_eq!(receipt.payload.payment_status, "settled");
    assert_eq!(receipt.algorithm, ED25519_ALGORITHM_NAME);
    assert_eq!(receipt.payload.prompt_tokens, Some(12));
    assert_eq!(receipt.payload.completion_tokens, Some(24));

    // Verify cryptographic signature
    let active_signer = AppState::receipt_signer().expect("active receipt signer");
    ReceiptSigner::verify_signature(&receipt, None).expect("verify receipt signature embedded key");
    ReceiptSigner::verify_signature(&receipt, Some(active_signer.public_key_hex()))
        .expect("verify receipt signature trusted key");

    // Exactly one receipt persisted
    let receipt_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM receipts WHERE id = ?1")
        .bind(&receipt_id_str)
        .fetch_one(ctx.db.pool())
        .await
        .unwrap();
    assert_eq!(receipt_count, 1);

    // Verify session accounting
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
}

#[tokio::test]
async fn test_non_streaming_receipt_persistence_failure_does_not_return_receipt_header_and_preserves_accounting(
) {
    use satsgrid::node::receipt::ReceiptSigner;

    let ctx = setup_test_context(UpstreamConfig::default()).await;
    let signer = Arc::new(
        ReceiptSigner::load_or_generate(ctx._dir.path(), "satsgrid-node").expect("signer"),
    );
    AppState::set_receipt_signer(signer);

    // Injected receipt persistence failure trigger
    sqlx::query(
        "CREATE TRIGGER reject_non_streaming_receipt BEFORE INSERT ON receipts BEGIN SELECT RAISE(FAIL, 'injected receipt persistence failure'); END",
    )
    .execute(ctx.db.pool())
    .await
    .unwrap();

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "cmpl_receipt_failure",
            "choices": [{"message": {"role": "assistant", "content": "Receipt failure test"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 20, "total_tokens": 30}
        })))
        .mount(&ctx.mock_server)
        .await;

    let session_uuid = uuid::Uuid::new_v4();
    let session_id = SessionId(session_uuid);
    let invoice = ctx
        .lightning
        .create_invoice(Satoshis(100), "Receipt persistence failure test")
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
    assert_eq!(resp.status(), StatusCode::OK);

    // CRITICAL: x-satsgrid-receipt-id header MUST NOT be present when persistence failed!
    assert!(
        resp.headers().get("x-satsgrid-receipt-id").is_none(),
        "x-satsgrid-receipt-id must NOT be returned when persistence fails"
    );

    // Verify response body still delivered the assistant response to client
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(
        body_json["choices"][0]["message"]["content"],
        "Receipt failure test"
    );

    // Verify no receipts were persisted in database
    let total_receipts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM receipts")
        .fetch_one(ctx.db.pool())
        .await
        .unwrap();
    assert_eq!(total_receipts, 0);

    // Verify session accounting invariants: exactly one debit of 10 sats occurred
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

    // Verify no duplicate debits occurred
    let debit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM accounting_entries WHERE session_id = ?1 AND entry_type = 'session_debit'",
    )
    .bind(session_id.0.to_string())
    .fetch_one(ctx.db.pool())
    .await
    .unwrap();
    assert_eq!(debit_count, 1, "Exactly one debit entry must exist");
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
        lightning: LightningConfig {
            allow_mock: true,
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
        concurrency_manager: Arc::new(satsgrid::node::concurrency::ConcurrencyManager::new(
            config.concurrency.max_concurrent_requests,
            config.concurrency.max_concurrent_per_session,
            config.concurrency.admission_timeout_secs,
        )),
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

#[tokio::test]
async fn test_streaming_receipt_delivery_and_openai_client_compatibility() {
    use satsgrid::node::receipt::{ReceiptSigner, ED25519_ALGORITHM_NAME};

    let ctx = setup_test_context(UpstreamConfig::default()).await;
    let signer = Arc::new(
        ReceiptSigner::load_or_generate(ctx._dir.path(), "satsgrid-node").expect("signer"),
    );
    AppState::set_receipt_signer(signer.clone());

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(
                    "data: {\"choices\":[{\"delta\":{\"content\":\"Hello \"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"world!\"}}]}\n\ndata: [DONE]\n\n",
                ),
        )
        .mount(&ctx.mock_server)
        .await;

    let session_uuid = uuid::Uuid::new_v4();
    let session_id = SessionId(session_uuid);
    let invoice = ctx
        .lightning
        .create_invoice(Satoshis(100), "Streaming receipt test")
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
                Caveat::ExpiresAt((chrono::Utc::now().timestamp() + 3600) as u64),
                Caveat::Model("llama3.2".to_string()),
            ],
        )
        .unwrap();

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::AUTHORIZATION,
            format!("L402 {}:{}", macaroon.to_base64().unwrap(), preimage),
        )
        .body(Body::from(
            json!({
                "model": "llama3.2",
                "messages": [{"role": "user", "content": "hi"}],
                "stream": true
            })
            .to_string(),
        ))
        .unwrap();

    let response = ctx.app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);

    // 1. Simulate an OpenAI-compatible client parsing the SSE stream
    let mut accumulated_text = String::new();
    let mut captured_receipt_id: Option<String> = None;
    let mut saw_done = false;

    for chunk in body_str.split("\n\n") {
        let trimmed = chunk.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lines: Vec<&str> = trimmed.lines().map(|l| l.trim()).collect();
        let mut data_str: Option<&str> = None;

        for line in lines {
            if let Some(rest) = line.strip_prefix("data:") {
                data_str = Some(rest.trim());
            }
        }

        if let Some(data) = data_str {
            if data == "[DONE]" {
                saw_done = true;
                break;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                // Check if chunk has delta content
                if let Some(content) = v
                    .get("choices")
                    .and_then(|c| c.get(0))
                    .and_then(|c0| c0.get("delta"))
                    .and_then(|d| d.get("content"))
                    .and_then(|s| s.as_str())
                {
                    accumulated_text.push_str(content);
                }
                // Check if chunk exposes receipt_id
                if let Some(rcpt) = v.get("receipt_id").and_then(|s| s.as_str()) {
                    captured_receipt_id = Some(rcpt.to_string());
                }
            }
        }
    }

    assert!(saw_done, "Stream must terminate with [DONE]");
    assert_eq!(
        accumulated_text, "Hello world!",
        "OpenAI client must accumulate all tokens"
    );
    assert!(
        captured_receipt_id.is_some(),
        "Stream must deliver receipt_id"
    );

    let receipt_id = captured_receipt_id.unwrap();
    assert!(receipt_id.starts_with("rcpt_"));

    // 2. Fetch and cryptographically verify the emitted receipt ID
    let receipt = ctx
        .db
        .receipt_store()
        .get_receipt(&receipt_id)
        .await
        .unwrap()
        .expect("receipt must exist in store");
    assert_eq!(receipt.payload.receipt_id, receipt_id);
    assert_eq!(receipt.payload.inference_status, "completed");
    assert_eq!(receipt.payload.payment_status, "settled");
    assert_eq!(receipt.algorithm, ED25519_ALGORITHM_NAME);
    ReceiptSigner::verify_signature(&receipt, None).expect("verify receipt signature");

    // Exactly one receipt persisted
    let receipt_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM receipts WHERE id = ?1")
        .bind(&receipt_id)
        .fetch_one(ctx.db.pool())
        .await
        .unwrap();
    assert_eq!(receipt_count, 1);

    // Invariant check: exactly one session debit
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
}
