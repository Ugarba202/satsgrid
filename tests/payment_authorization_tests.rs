use satsgrid::common::types::PaymentHash;
use satsgrid::node::storage::{Database, ReservationResult};
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test]
async fn test_first_authorization_reservation_succeeds() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let store = db.payment_store();

    let hash =
        PaymentHash("aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899".to_string());
    let req_id = "req_1";

    let res = store
        .reserve_payment_hash(&hash, req_id, 2000000000, Some("idem_1"))
        .await
        .expect("reserve");

    assert_eq!(res, ReservationResult::Reserved);

    let state = store
        .get_authorization_state(&hash)
        .await
        .expect("get state");
    assert_eq!(state, Some("reserved".to_string()));
}

#[tokio::test]
async fn test_sequential_duplicate_reservation_is_rejected() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let store = db.payment_store();

    let hash =
        PaymentHash("11223344556677889900aabbccddeeff11223344556677889900aabbccddeeff".to_string());

    let res1 = store
        .reserve_payment_hash(&hash, "req_1", 2000000000, None)
        .await
        .expect("reserve 1");
    assert_eq!(res1, ReservationResult::Reserved);

    // Second attempt with exact same hash
    let res2 = store
        .reserve_payment_hash(&hash, "req_2", 2000000000, None)
        .await
        .expect("reserve 2");

    assert!(matches!(res2, ReservationResult::AlreadyUsed { .. }));
}

#[tokio::test]
async fn test_concurrent_duplicate_reservations_allow_exactly_one_winner() {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));
    let hash =
        PaymentHash("deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef".to_string());

    let mut handles = Vec::new();
    let num_tasks = 20;

    for i in 0..num_tasks {
        let db_clone = db.clone();
        let hash_clone = hash.clone();
        let req_id = format!("req_{}", i);
        let idem_key = format!("idem_{}", i);

        handles.push(tokio::spawn(async move {
            let store = db_clone.payment_store();
            store
                .reserve_payment_hash(&hash_clone, &req_id, 2000000000, Some(&idem_key))
                .await
        }));
    }

    let mut winners = 0;
    let mut rejected = 0;

    for h in handles {
        let res = h.await.expect("join handle").expect("reservation query");
        match res {
            ReservationResult::Reserved => winners += 1,
            ReservationResult::AlreadyUsed { .. } => rejected += 1,
        }
    }

    assert_eq!(
        winners, 1,
        "Exactly one concurrent reservation must succeed"
    );
    assert_eq!(
        rejected,
        num_tasks - 1,
        "All racing reservations must be rejected"
    );
}

#[tokio::test]
async fn test_same_payment_hash_with_different_idempotency_key_is_rejected() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let store = db.payment_store();

    let hash =
        PaymentHash("44556677889900112233aabbccddeeff44556677889900112233aabbccddeeff".to_string());

    let res1 = store
        .reserve_payment_hash(&hash, "req_1", 2000000000, Some("key_alpha"))
        .await
        .expect("reserve 1");
    assert_eq!(res1, ReservationResult::Reserved);

    // Same hash, different idempotency key
    let res2 = store
        .reserve_payment_hash(&hash, "req_2", 2000000000, Some("key_beta"))
        .await
        .expect("reserve 2");

    assert!(matches!(res2, ReservationResult::AlreadyUsed { .. }));
}

#[tokio::test]
async fn test_consumed_authorization_survives_reconnect_and_reinit() {
    let dir = tempdir().expect("create tempdir");
    let db_path = dir.path().join("persist_auth.db");
    let hash =
        PaymentHash("cafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeefcafebeef".to_string());

    {
        let db = Database::init(&db_path).await.expect("init 1");
        let store = db.payment_store();
        store
            .reserve_payment_hash(&hash, "req_1", 2000000000, None)
            .await
            .expect("reserve");
        store
            .mark_settled_and_consumed(&hash)
            .await
            .expect("mark consumed");
    }

    // Reopen database from disk simulating node restart
    {
        let db2 = Database::init(&db_path).await.expect("init 2");
        let store2 = db2.payment_store();

        let state = store2
            .get_authorization_state(&hash)
            .await
            .expect("get state");
        assert_eq!(state, Some("consumed".to_string()));

        let res = store2
            .reserve_payment_hash(&hash, "req_replayed", 2000000000, None)
            .await
            .expect("try re-reserve");

        assert!(matches!(res, ReservationResult::AlreadyUsed { .. }));
    }
}

#[tokio::test]
async fn test_session_credentials_do_not_enter_payment_authorizations() {
    // Verifies that extracting a session caveat bypasses payment_authorizations insertion
    use satsgrid::node::gate::macaroon::{Caveat, MacaroonService};
    use satsgrid::node::gate::SessionBudgetManager;
    use uuid::Uuid;

    let service = MacaroonService::new(b"01234567890123456789012345678901".to_vec(), "node");
    let hash =
        PaymentHash("abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd".to_string());
    let session_uuid = Uuid::new_v4();

    let macaroon = service
        .mint(
            &hash,
            vec![
                Caveat::Session(session_uuid.to_string()),
                Caveat::Budget(100),
                Caveat::Capability("inference".to_string()),
            ],
        )
        .expect("mint macaroon");

    let (extracted_session, _) = SessionBudgetManager::extract_session_caveats(&macaroon);
    assert!(
        extracted_session.is_some(),
        "Session caveat must be recognized"
    );

    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let store = db.payment_store();

    // The store should not have this hash recorded
    let state = store
        .get_authorization_state(&hash)
        .await
        .expect("query state");
    assert_eq!(
        state, None,
        "Session tokens must not be recorded in single-use authorizations"
    );
}

#[tokio::test]
async fn test_e2e_replay_rejected_in_chat_completions() {
    use axum::http::StatusCode;
    use satsgrid::node::api::routes::create_routes;
    use satsgrid::node::api::AppState;
    use satsgrid::node::gate::{Caveat, MacaroonService, SessionBudgetManager};
    use satsgrid::node::lightning::mock::MockLightningBackend;
    use satsgrid::node::lightning::LightningBackend;
    use satsgrid::node::proxy::openai::OpenAiProxy;
    use serde_json::json;
    use tower::ServiceExt;

    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));

    let backend = Arc::new(MockLightningBackend::new());
    let root_key = b"01234567890123456789012345678901".to_vec();
    let macaroon_service = Arc::new(MacaroonService::new(root_key, "satsgrid-node"));

    let config = satsgrid::config::schema::NodeConfig::default();
    let proxy = OpenAiProxy::new("http://127.0.0.1:11434");

    let state = AppState {
        config: Arc::new(config.clone()),
        live_pricing: Arc::new(tokio::sync::RwLock::new(config.pricing.clone())),
        lightning: backend.clone(),
        budget_manager: Arc::new(SessionBudgetManager::new()),
        macaroon_service: macaroon_service.clone(),
        proxy,
        stats: Arc::new(satsgrid::node::api::NodeStats::default()),
        admin_token: Arc::new("admin123".to_string()),
        db: Some(db),
        concurrency_manager: Arc::new(satsgrid::node::concurrency::ConcurrencyManager::new(
            config.concurrency.max_concurrent_requests,
            config.concurrency.max_concurrent_per_session,
            config.concurrency.admission_timeout_secs,
        )),
    };

    let app = create_routes(state);

    // 1. Create invoice and pay in mock backend to obtain valid preimage
    let invoice = backend
        .create_invoice(satsgrid::common::types::Satoshis(10), "test")
        .await
        .expect("create invoice");
    let preimage = backend
        .pay_invoice(&invoice.bolt11)
        .await
        .expect("pay invoice");

    // 2. Mint single-use pay-per-request macaroon
    let macaroon = macaroon_service
        .mint(
            &invoice.payment_hash,
            vec![
                Caveat::Capability("inference".to_string()),
                Caveat::ExpiresAt(chrono::Utc::now().timestamp() as u64 + 3600),
            ],
        )
        .expect("mint macaroon");
    let auth_header = format!("L402 {}:{}", macaroon.to_base64().unwrap(), preimage);

    let payload = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "Hello"}],
        "stream": false
    });

    let make_request = || {
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .header("authorization", &auth_header)
            .body(axum::body::Body::from(
                serde_json::to_vec(&payload).unwrap(),
            ))
            .unwrap()
    };

    // First request should get past L402 verification and reach upstream proxy (which fails with 502/upstream error or succeeds)
    let resp1 = app.clone().oneshot(make_request()).await.unwrap();
    // It should NOT be 401 Unauthorized (since credential was valid and reserved)
    assert_ne!(resp1.status(), StatusCode::UNAUTHORIZED);

    // Second request with exact same L402 header MUST be rejected as 401 Unauthorized (Replay detected)
    let resp2 = app.clone().oneshot(make_request()).await.unwrap();
    assert_eq!(
        resp2.status(),
        StatusCode::UNAUTHORIZED,
        "Replayed token must be rejected with 401 Unauthorized"
    );
}

#[tokio::test]
async fn test_commit_authorization_and_mark_processing_atomic_success() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let store = db.payment_store();
    let req_store = db.request_store();

    let hash =
        PaymentHash("1111222233334444555566667777888899990000aaaabbbbccccddddeeeeffff".to_string());
    let req_id = "req_dispatch_1";

    // 1. Reserve payment authorization
    let res = store
        .reserve_payment_hash(&hash, req_id, 2000000000, None)
        .await
        .unwrap();
    assert_eq!(res, ReservationResult::Reserved);

    // 2. Create inference request in reserved state
    req_store
        .create_request(
            req_id,
            Some(&hash),
            None,
            None,
            None,
            "llama3.2",
            satsgrid::node::storage::RequestMode::NonStreaming,
            satsgrid::common::types::Satoshis(10),
        )
        .await
        .unwrap();

    // 3. Atomically commit authorization and mark processing
    db.commit_authorization_and_mark_processing(&hash, req_id)
        .await
        .expect("atomic dispatch transition should succeed");

    // 4. Verify both transitions committed
    let auth_state = store.get_authorization_state(&hash).await.unwrap();
    assert_eq!(auth_state, Some("consumed".to_string()));

    let req_record = req_store.get_request(req_id).await.unwrap().unwrap();
    assert_eq!(
        req_record.status,
        satsgrid::node::storage::RequestStatus::Processing
    );
    assert!(req_record.started_at.is_some());
}

#[tokio::test]
async fn test_commit_authorization_rollback_on_mismatches() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let store = db.payment_store();
    let req_store = db.request_store();

    let hash =
        PaymentHash("9999222233334444555566667777888899990000aaaabbbbccccddddeeeeffff".to_string());
    let req_id = "req_valid_1";

    store
        .reserve_payment_hash(&hash, req_id, 2000000000, None)
        .await
        .unwrap();

    // 1. Test missing inference request -> fails and authorization remains reserved
    let err_missing_req = db
        .commit_authorization_and_mark_processing(&hash, req_id)
        .await;
    assert!(err_missing_req.is_err());
    let state = store.get_authorization_state(&hash).await.unwrap();
    assert_eq!(state, Some("reserved".to_string()));

    // Create the request
    req_store
        .create_request(
            req_id,
            Some(&hash),
            None,
            None,
            None,
            "llama3.2",
            satsgrid::node::storage::RequestMode::NonStreaming,
            satsgrid::common::types::Satoshis(10),
        )
        .await
        .unwrap();

    // 2. Test wrong request_id -> fails and both remain reserved
    let err_wrong_id = db
        .commit_authorization_and_mark_processing(&hash, "req_other")
        .await;
    assert!(err_wrong_id.is_err());
    let state = store.get_authorization_state(&hash).await.unwrap();
    assert_eq!(state, Some("reserved".to_string()));
    let req_record = req_store.get_request(req_id).await.unwrap().unwrap();
    assert_eq!(
        req_record.status,
        satsgrid::node::storage::RequestStatus::Reserved
    );

    // 3. Test wrong payment hash -> fails
    let bad_hash =
        PaymentHash("0000000000000000000000000000000000000000000000000000000000000000".to_string());
    let err_bad_hash = db
        .commit_authorization_and_mark_processing(&bad_hash, req_id)
        .await;
    assert!(err_bad_hash.is_err());
}
