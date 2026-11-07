use satsgrid::common::types::{PaymentHash, Satoshis, SessionId};
use satsgrid::node::storage::{Database, RequestMode, RequestStatus};
use std::sync::Arc;
use tempfile::tempdir;
use uuid::Uuid;

#[tokio::test]
async fn test_request_creation_and_retrieval() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let pay_store = db.payment_store();
    let store = db.request_store();

    let req_id = Uuid::new_v4().to_string();
    let payment_hash =
        PaymentHash("aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899".to_string());

    pay_store
        .reserve_payment_hash(&payment_hash, &req_id, 2000000000, Some("idem_1"))
        .await
        .unwrap();

    store
        .create_request(
            &req_id,
            Some(&payment_hash),
            None,
            None,
            Some("idem_1"),
            "llama3.2",
            RequestMode::NonStreaming,
            Satoshis(15),
        )
        .await
        .expect("create request");

    let record = store
        .get_request(&req_id)
        .await
        .expect("get request")
        .expect("must exist");

    assert_eq!(record.id, req_id);
    assert_eq!(record.payment_hash, Some(payment_hash));
    assert_eq!(record.session_id, None);
    assert_eq!(record.idempotency_key, Some("idem_1".to_string()));
    assert_eq!(record.model, "llama3.2");
    assert_eq!(record.request_mode, RequestMode::NonStreaming);
    assert_eq!(record.status, RequestStatus::Reserved);
    assert_eq!(record.charged_sats, Satoshis(15));
    assert_eq!(record.started_at, None);
    assert_eq!(record.completed_at, None);
}

#[tokio::test]
async fn test_valid_state_transitions() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let pay_store = db.payment_store();
    let store = db.request_store();

    let req_id = Uuid::new_v4().to_string();
    let payment_hash =
        PaymentHash("11223344556677889900aabbccddeeff11223344556677889900aabbccddeeff".to_string());

    pay_store
        .reserve_payment_hash(&payment_hash, &req_id, 2000000000, None)
        .await
        .unwrap();

    // 1. Created in 'reserved'
    store
        .create_request(
            &req_id,
            Some(&payment_hash),
            None,
            None,
            None,
            "llama3.2",
            RequestMode::NonStreaming,
            Satoshis(20),
        )
        .await
        .expect("create");

    // 2. Reserved -> Processing (dispatch marker)
    store
        .mark_processing(&req_id)
        .await
        .expect("mark processing");
    let rec_proc = store.get_request(&req_id).await.unwrap().unwrap();
    assert_eq!(rec_proc.status, RequestStatus::Processing);
    assert!(rec_proc.started_at.is_some());

    // 3. Processing -> Completed
    store
        .mark_completed(&req_id, Some(10), Some(40))
        .await
        .expect("mark completed");
    let rec_comp = store.get_request(&req_id).await.unwrap().unwrap();
    assert_eq!(rec_comp.status, RequestStatus::Completed);
    assert!(rec_comp.completed_at.is_some());
    assert_eq!(rec_comp.actual_prompt_tokens, Some(10));
    assert_eq!(rec_comp.actual_completion_tokens, Some(40));
}

#[tokio::test]
async fn test_invalid_state_transition_rejection() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let sess_store = db.session_store();
    let store = db.request_store();

    let req_id = Uuid::new_v4().to_string();
    let session_id = SessionId(Uuid::new_v4());
    sess_store
        .create_session(&session_id, None, Satoshis(100), 2000000000)
        .await
        .unwrap();
    sess_store
        .reserve_budget(&session_id, Satoshis(10), "res_invalid_tx")
        .await
        .unwrap();

    store
        .create_request(
            &req_id,
            None,
            Some(&session_id),
            Some("res_invalid_tx"),
            None,
            "llama3.2",
            RequestMode::NonStreaming,
            Satoshis(10),
        )
        .await
        .expect("create");

    // Cannot jump straight from reserved to completed (must pass processing)
    let comp_res = store.mark_completed(&req_id, None, None).await;
    assert!(comp_res.is_err());
    assert!(comp_res
        .unwrap_err()
        .to_string()
        .contains("Cannot mark request"));

    // Verify status is still reserved
    let rec = store.get_request(&req_id).await.unwrap().unwrap();
    assert_eq!(rec.status, RequestStatus::Reserved);
}

#[tokio::test]
async fn test_duplicate_transition_rejection() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let sess_store = db.session_store();
    let store = db.request_store();

    let req_id = Uuid::new_v4().to_string();
    let session_id = SessionId(Uuid::new_v4());
    sess_store
        .create_session(&session_id, None, Satoshis(100), 2000000000)
        .await
        .unwrap();
    sess_store
        .reserve_budget(&session_id, Satoshis(10), "res_dup_tx")
        .await
        .unwrap();

    store
        .create_request(
            &req_id,
            None,
            Some(&session_id),
            Some("res_dup_tx"),
            None,
            "llama3.2",
            RequestMode::NonStreaming,
            Satoshis(10),
        )
        .await
        .expect("create");

    store
        .mark_processing(&req_id)
        .await
        .expect("mark processing 1");

    // Second processing attempt must fail
    let proc2 = store.mark_processing(&req_id).await;
    assert!(proc2.is_err());

    store
        .mark_completed(&req_id, None, None)
        .await
        .expect("mark completed 1");

    // Second completed attempt must fail
    let comp2 = store.mark_completed(&req_id, None, None).await;
    assert!(comp2.is_err());
}

#[tokio::test]
async fn test_payment_request_linkage() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let payment_store = db.payment_store();
    let request_store = db.request_store();

    let payment_hash =
        PaymentHash("99887766554433221100aabbccddeeff99887766554433221100aabbccddeeff".to_string());
    let req_id = Uuid::new_v4().to_string();

    // 1. Reserve payment authorization
    payment_store
        .reserve_payment_hash(&payment_hash, &req_id, 2000000000, None)
        .await
        .expect("reserve payment");

    // 2. Link to request
    request_store
        .create_request(
            &req_id,
            Some(&payment_hash),
            None,
            None,
            None,
            "llama3.2",
            RequestMode::NonStreaming,
            Satoshis(50),
        )
        .await
        .expect("create request linked to payment");

    let rec = request_store.get_request(&req_id).await.unwrap().unwrap();
    assert_eq!(rec.payment_hash, Some(payment_hash.clone()));

    // Verify foreign key constraint: an unknown payment hash fails
    let fake_hash =
        PaymentHash("0000000000000000000000000000000000000000000000000000000000000000".to_string());
    let fake_res = request_store
        .create_request(
            &Uuid::new_v4().to_string(),
            Some(&fake_hash),
            None,
            None,
            None,
            "llama3.2",
            RequestMode::NonStreaming,
            Satoshis(50),
        )
        .await;
    assert!(
        fake_res.is_err(),
        "Foreign key must reject unregistered payment hash"
    );
}

#[tokio::test]
async fn test_session_reservation_linkage() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let session_store = db.session_store();
    let request_store = db.request_store();

    let session_id = SessionId(Uuid::new_v4());
    let res_id = "res_sess_link_1";
    let req_id = Uuid::new_v4().to_string();

    session_store
        .create_session(&session_id, None, Satoshis(100), 2000000000)
        .await
        .expect("create session");

    session_store
        .reserve_budget(&session_id, Satoshis(40), res_id)
        .await
        .expect("reserve");

    request_store
        .create_request(
            &req_id,
            None,
            Some(&session_id),
            Some(res_id),
            Some("key_sess_1"),
            "llama3.2",
            RequestMode::NonStreaming,
            Satoshis(40),
        )
        .await
        .expect("create linked request");

    let rec = request_store.get_request(&req_id).await.unwrap().unwrap();
    assert_eq!(rec.session_id, Some(session_id));
    assert_eq!(rec.reservation_id, Some(res_id.to_string()));
}

#[tokio::test]
async fn test_duplicate_session_idempotency_prevention() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let session_store = db.session_store();
    let request_store = db.request_store();

    let session_id = SessionId(Uuid::new_v4());
    session_store
        .create_session(&session_id, None, Satoshis(100), 2000000000)
        .await
        .expect("create session");
    session_store
        .reserve_budget(&session_id, Satoshis(25), "res_k1")
        .await
        .unwrap();

    let req_id = Uuid::new_v4().to_string();
    request_store
        .create_request(
            &req_id,
            None,
            Some(&session_id),
            Some("res_k1"),
            Some("idem_k1"),
            "llama3.2",
            RequestMode::NonStreaming,
            Satoshis(25),
        )
        .await
        .expect("create request 1");

    store_processing_and_complete(&request_store, &req_id).await;

    // Check duplicate detection
    let existing = request_store
        .find_by_idempotency_key(&session_id, "idem_k1")
        .await
        .expect("lookup")
        .expect("should find");

    assert_eq!(existing.status, RequestStatus::Completed);
}

async fn store_processing_and_complete(store: &satsgrid::node::storage::RequestStore, id: &str) {
    store.mark_processing(id).await.unwrap();
    store.mark_completed(id, Some(5), Some(15)).await.unwrap();
}

#[tokio::test]
async fn test_crash_before_dispatch_recovery() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let session_store = db.session_store();
    let request_store = db.request_store();

    let session_id = SessionId(Uuid::new_v4());
    session_store
        .create_session(&session_id, None, Satoshis(100), 2000000000)
        .await
        .unwrap();

    let res_id = "res_crash_pre";
    session_store
        .reserve_budget(&session_id, Satoshis(50), res_id)
        .await
        .unwrap();

    let req_id = Uuid::new_v4().to_string();
    request_store
        .create_request(
            &req_id,
            None,
            Some(&session_id),
            Some(res_id),
            None,
            "llama3.2",
            RequestMode::NonStreaming,
            Satoshis(50),
        )
        .await
        .unwrap();

    // Node crashes BEFORE mark_processing (started_at is NULL)
    // Run recovery
    let undispatched = request_store.recover_in_flight_requests().await.unwrap();
    assert_eq!(undispatched.len(), 1);
    assert_eq!(undispatched[0].0, res_id);

    // Release reservation
    let undispatched_ids: Vec<String> = undispatched.into_iter().map(|(r, _)| r).collect();
    let recovered_count = session_store
        .recover_in_flight_reservations(&undispatched_ids)
        .await
        .unwrap();
    assert_eq!(recovered_count, 1);

    // Verify session budget was restored
    let sess = session_store
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sess.remaining_sats, Satoshis(100));
    assert_eq!(sess.reserved_sats, Satoshis(0));

    // Verify request state is failed
    let req = request_store.get_request(&req_id).await.unwrap().unwrap();
    assert_eq!(req.status, RequestStatus::Failed);
    assert_eq!(
        req.error_code,
        Some("server_crash_prior_to_dispatch".to_string())
    );
}

#[tokio::test]
async fn test_crash_after_dispatch_marker_retention() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let session_store = db.session_store();
    let request_store = db.request_store();

    let session_id = SessionId(Uuid::new_v4());
    session_store
        .create_session(&session_id, None, Satoshis(100), 2000000000)
        .await
        .unwrap();

    let res_id = "res_crash_post";
    session_store
        .reserve_budget(&session_id, Satoshis(50), res_id)
        .await
        .unwrap();

    let req_id = Uuid::new_v4().to_string();
    request_store
        .create_request(
            &req_id,
            None,
            Some(&session_id),
            Some(res_id),
            None,
            "llama3.2",
            RequestMode::NonStreaming,
            Satoshis(50),
        )
        .await
        .unwrap();

    // Dispatch marker persisted (started_at IS NOT NULL)
    request_store.mark_processing(&req_id).await.unwrap();

    // Node crashes while request is in flight upstream
    let undispatched = request_store.recover_in_flight_requests().await.unwrap();
    assert_eq!(
        undispatched.len(),
        0,
        "Dispatched request must NOT be in undispatched list"
    );

    // Session recovery marks ambiguous reservations as reconciliation_required
    session_store
        .recover_in_flight_reservations(&[])
        .await
        .unwrap();

    // Balance must NOT be refunded!
    let sess = session_store
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sess.remaining_sats, Satoshis(50));
    assert_eq!(sess.reserved_sats, Satoshis(50));

    let req = request_store.get_request(&req_id).await.unwrap().unwrap();
    assert_eq!(req.status, RequestStatus::ReconciliationRequired);
    assert!(req.reconciliation_required);
}

#[tokio::test]
async fn test_failed_upstream_request_releases_reservation() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let session_store = db.session_store();
    let request_store = db.request_store();

    let session_id = SessionId(Uuid::new_v4());
    session_store
        .create_session(&session_id, None, Satoshis(100), 2000000000)
        .await
        .unwrap();

    let res_id = "res_upstream_fail";
    session_store
        .reserve_budget(&session_id, Satoshis(30), res_id)
        .await
        .unwrap();

    let req_id = Uuid::new_v4().to_string();
    request_store
        .create_request(
            &req_id,
            None,
            Some(&session_id),
            Some(res_id),
            None,
            "llama3.2",
            RequestMode::NonStreaming,
            Satoshis(30),
        )
        .await
        .unwrap();

    request_store.mark_processing(&req_id).await.unwrap();

    // Upstream returns 500 / timeout -> mark failed and release
    request_store
        .mark_failed(&req_id, "upstream_500", "upstream_error")
        .await
        .unwrap();
    session_store
        .release_reservation(res_id, "upstream_failure")
        .await
        .unwrap();

    let sess = session_store
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sess.remaining_sats, Satoshis(100));
    assert_eq!(sess.reserved_sats, Satoshis(0));

    let req = request_store.get_request(&req_id).await.unwrap().unwrap();
    assert_eq!(req.status, RequestStatus::Failed);
    assert_eq!(req.error_code, Some("upstream_500".to_string()));
}

#[tokio::test]
async fn test_sse_completion_and_interruption() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let sess_store = db.session_store();
    let store = db.request_store();

    // 1. Successful stream completion
    let req_1 = Uuid::new_v4().to_string();
    let sid_1 = SessionId(Uuid::new_v4());
    sess_store
        .create_session(&sid_1, None, Satoshis(100), 2000000000)
        .await
        .unwrap();
    sess_store
        .reserve_budget(&sid_1, Satoshis(50), "res_s1")
        .await
        .unwrap();

    store
        .create_request(
            &req_1,
            None,
            Some(&sid_1),
            Some("res_s1"),
            None,
            "llama3.2",
            RequestMode::Streaming,
            Satoshis(50),
        )
        .await
        .unwrap();
    store.mark_processing(&req_1).await.unwrap();
    store.mark_completed(&req_1, Some(15), None).await.unwrap();

    let rec1 = store.get_request(&req_1).await.unwrap().unwrap();
    assert_eq!(rec1.status, RequestStatus::Completed);
    assert_eq!(rec1.request_mode, RequestMode::Streaming);

    // 2. Stream interrupted mid-flight
    let req_2 = Uuid::new_v4().to_string();
    let sid_2 = SessionId(Uuid::new_v4());
    sess_store
        .create_session(&sid_2, None, Satoshis(100), 2000000000)
        .await
        .unwrap();
    sess_store
        .reserve_budget(&sid_2, Satoshis(50), "res_s2")
        .await
        .unwrap();

    store
        .create_request(
            &req_2,
            None,
            Some(&sid_2),
            Some("res_s2"),
            None,
            "llama3.2",
            RequestMode::Streaming,
            Satoshis(50),
        )
        .await
        .unwrap();
    store.mark_processing(&req_2).await.unwrap();
    store
        .mark_stream_interrupted(&req_2, "client_disconnected", Some(25))
        .await
        .unwrap();

    let rec2 = store.get_request(&req_2).await.unwrap().unwrap();
    assert_eq!(rec2.status, RequestStatus::StreamInterrupted);
    assert_eq!(rec2.actual_completion_tokens, Some(25));
    assert_eq!(rec2.error_code, Some("client_disconnected".to_string()));
}

#[tokio::test]
async fn test_concurrent_request_creation() {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));
    let sess_store = db.session_store();

    let sid = SessionId(Uuid::new_v4());
    sess_store
        .create_session(&sid, None, Satoshis(1000), 2000000000)
        .await
        .unwrap();

    let num_tasks = 10;
    let mut handles = Vec::new();

    for i in 0..num_tasks {
        let db_clone = db.clone();
        let s_clone = sid.clone();
        handles.push(tokio::spawn(async move {
            let req_id = format!("req_concurrent_{}", i);
            let store = db_clone.request_store();
            store
                .create_request(
                    &req_id,
                    None,
                    Some(&s_clone),
                    None,
                    None,
                    "llama3.2",
                    RequestMode::NonStreaming,
                    Satoshis(10),
                )
                .await
        }));
    }

    for h in handles {
        h.await.unwrap().expect("create concurrent request");
    }

    // Verify all 10 requests were recorded
    for i in 0..num_tasks {
        let req_id = format!("req_concurrent_{}", i);
        let rec = db.request_store().get_request(&req_id).await.unwrap();
        assert!(rec.is_some());
    }
}

#[tokio::test]
async fn test_concurrent_duplicate_session_idempotency_keys_rejected() {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));
    let sess_store = db.session_store();

    let sid = SessionId(Uuid::new_v4());
    sess_store
        .create_session(&sid, None, Satoshis(1000), 2000000000)
        .await
        .unwrap();

    let idempotency_key = "concurrent_idem_unique_test";
    let num_tasks = 8;
    let mut handles = Vec::new();

    for i in 0..num_tasks {
        let db_clone = db.clone();
        let s_clone = sid.clone();
        handles.push(tokio::spawn(async move {
            let req_id = format!("req_idem_{}", i);
            let store = db_clone.request_store();
            store
                .create_request(
                    &req_id,
                    None,
                    Some(&s_clone),
                    None,
                    Some(idempotency_key),
                    "llama3.2",
                    RequestMode::NonStreaming,
                    Satoshis(10),
                )
                .await
        }));
    }

    let mut success_count = 0;
    let mut failure_count = 0;

    for h in handles {
        match h.await.unwrap() {
            Ok(_) => success_count += 1,
            Err(_) => failure_count += 1,
        }
    }

    // Exactly one insertion with (session_id, idempotency_key) must win
    assert_eq!(
        success_count, 1,
        "Exactly one request must succeed with the same session idempotency key"
    );
    assert_eq!(
        failure_count,
        num_tasks - 1,
        "All other racing requests must be rejected by unique index"
    );
}

#[tokio::test]
async fn test_sse_early_disconnect_marks_stream_interrupted_not_completed() {
    use axum::body::Body;
    use http::{Request, StatusCode};
    use satsgrid::config::schema::{NodeConfig, PricingConfig};
    use satsgrid::node::api::routes::create_routes;
    use satsgrid::node::api::AppState;
    use satsgrid::node::gate::{MacaroonService, SessionBudgetManager};
    use satsgrid::node::lightning::mock::MockLightningBackend;
    use satsgrid::node::proxy::openai::OpenAiProxy;
    use serde_json::json;
    use tower::ServiceExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));

    let mock_server = MockServer::start().await;

    // Upstream returns an SSE stream that produces chunks indefinitely or delays
    let sse_body = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n";
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .mount(&mock_server)
        .await;

    let config = NodeConfig {
        server: satsgrid::config::schema::ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            admin_token: None,
            ..Default::default()
        },
        pricing: PricingConfig::new(Satoshis(10)),
        upstream: satsgrid::config::schema::UpstreamConfig {
            url: mock_server.uri(),
        },
        lightning: satsgrid::config::schema::LightningConfig::default(),
        data_dir: dir.path().to_string_lossy().to_string(),
        ..Default::default()
    };

    let proxy = OpenAiProxy::new(config.upstream.url.clone());
    let session_store = db.session_store();
    let budget_manager = Arc::new(SessionBudgetManager::with_store(session_store.clone()));

    let sid = SessionId(Uuid::new_v4());
    session_store
        .create_session(&sid, None, Satoshis(500), 2000000000)
        .await
        .unwrap();

    let macaroon_service = Arc::new(MacaroonService::new(
        b"test-secret-key-0000000000000000".to_vec(),
        "satsgrid-node",
    ));

    let preimage_bytes = [0x5au8; 32];
    let preimage_hex = hex::encode(preimage_bytes);
    let known_payment_hash =
        satsgrid::node::gate::verify::L402Verifier::hash_preimage(&preimage_hex).unwrap();

    let mock_lightning = Arc::new(MockLightningBackend::new());
    mock_lightning.simulate_payment(&known_payment_hash).await;

    let macaroon = macaroon_service
        .mint(
            &known_payment_hash,
            vec![
                satsgrid::node::gate::macaroon::Caveat::Session(sid.0.to_string()),
                satsgrid::node::gate::macaroon::Caveat::Budget(500),
                satsgrid::node::gate::macaroon::Caveat::Capability("inference".to_string()),
                satsgrid::node::gate::macaroon::Caveat::ExpiresAt(2000000000),
            ],
        )
        .unwrap();

    let state = AppState {
        config: Arc::new(config),
        live_pricing: Arc::new(tokio::sync::RwLock::new(PricingConfig::new(Satoshis(10)))),
        lightning: mock_lightning,
        budget_manager,
        macaroon_service,
        proxy,
        stats: Arc::new(satsgrid::node::api::NodeStats::default()),
        admin_token: Arc::new("test-token".to_string()),
        db: Some(db.clone()),
    };

    let app = create_routes(state);

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!("L402 {}:{}", macaroon.to_base64().unwrap(), preimage_hex),
        )
        .body(Body::from(
            json!({
                "model": "llama3.2",
                "stream": true,
                "messages": [{"role": "user", "content": "hi"}]
            })
            .to_string(),
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Drop the response body immediately without consuming the stream (simulates client abrupt disconnect)
    drop(resp);

    // Wait a brief moment for asynchronous drop handler task to execute
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Verify in database: the request must NOT be 'completed'
    let _in_flight_rows = db
        .request_store()
        .recover_in_flight_requests()
        .await
        .unwrap();
    // Because it was marked stream_interrupted or reconciliation_required, it cannot be completed
    let requests: Vec<(String, String)> =
        sqlx::query_as("SELECT id, status FROM inference_requests")
            .fetch_all(db.pool())
            .await
            .unwrap();

    assert_eq!(requests.len(), 1);
    let (_req_id, status) = &requests[0];
    assert_ne!(
        status, "completed",
        "Abruptly dropped stream must NOT be marked completed!"
    );
    assert!(
        status == "stream_interrupted" || status == "reconciliation_required",
        "Dropped stream must be recorded as stream_interrupted or reconciliation_required, got {}",
        status
    );
}
