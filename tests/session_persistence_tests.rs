use satsgrid::common::types::{PaymentHash, Satoshis, SessionId};
use satsgrid::node::gate::budget::SessionBudgetManager;
use satsgrid::node::gate::macaroon::{Caveat, Macaroon, MacaroonService};
use satsgrid::node::storage::{Database, SessionReservationResult, SessionStatus};
use std::sync::Arc;
use tempfile::tempdir;
use uuid::Uuid;

#[tokio::test]
async fn test_session_creation_and_retrieval() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let store = db.session_store();

    let session_id = SessionId(Uuid::new_v4());
    let payment_hash =
        PaymentHash("11223344556677889900aabbccddeeff11223344556677889900aabbccddeeff".to_string());
    let budget = Satoshis(500);
    let expires_at = chrono::Utc::now().timestamp() + 3600;

    store
        .create_session(&session_id, Some(&payment_hash), budget, expires_at)
        .await
        .expect("create session");

    let record = store
        .get_session(&session_id)
        .await
        .expect("query session")
        .expect("session must exist");

    assert_eq!(record.id, session_id);
    assert_eq!(record.payment_hash, Some(payment_hash));
    assert_eq!(record.total_budget_sats, Satoshis(500));
    assert_eq!(record.remaining_sats, Satoshis(500));
    assert_eq!(record.reserved_sats, Satoshis(0));
    assert_eq!(record.spent_sats, Satoshis(0));
    assert_eq!(record.status, SessionStatus::Active);

    // Verify funding ledger event
    let entries = store
        .get_accounting_entries(&session_id)
        .await
        .expect("fetch entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].1, "session_funding");
    assert_eq!(entries[0].2, 500);
}

#[tokio::test]
async fn test_session_persistence_across_reconnect() {
    let dir = tempdir().expect("create tempdir");
    let db_path = dir.path().join("persist_session.db");
    let session_id = SessionId(Uuid::new_v4());
    let budget = Satoshis(1000);

    {
        let db1 = Database::init(&db_path).await.expect("init db1");
        let store1 = db1.session_store();
        store1
            .create_session(
                &session_id,
                None,
                budget,
                chrono::Utc::now().timestamp() + 3600,
            )
            .await
            .expect("create");
    }

    // Reopen database from disk
    {
        let db2 = Database::init(&db_path).await.expect("init db2");
        let store2 = db2.session_store();
        let record = store2
            .get_session(&session_id)
            .await
            .expect("get session")
            .expect("must exist");

        assert_eq!(record.id, session_id);
        assert_eq!(record.remaining_sats, Satoshis(1000));
        assert_eq!(record.status, SessionStatus::Active);
    }
}

#[tokio::test]
async fn test_session_persistence_across_node_reinitialization() {
    let dir = tempdir().expect("create tempdir");
    let db_path = dir.path().join("node_reinit.db");
    let session_id = SessionId(Uuid::new_v4());

    {
        let db = Database::init(&db_path).await.expect("init 1");
        let manager = SessionBudgetManager::with_store(db.session_store());
        manager.open_session(&session_id, Satoshis(250)).await;

        let rem = manager
            .debit_session(&session_id, Satoshis(50))
            .await
            .expect("debit 50");
        assert_eq!(rem, Satoshis(200));
    }

    // Simulate node restart with new SessionBudgetManager attached to same DB
    {
        let db = Database::init(&db_path).await.expect("init 2");
        let manager2 = SessionBudgetManager::with_store(db.session_store());

        let session = manager2
            .get_session(&session_id)
            .await
            .expect("session must exist in restarted manager");
        assert_eq!(session.remaining(), Satoshis(200));
        assert_eq!(session.total_sats, Satoshis(250));
    }
}

#[tokio::test]
async fn test_successful_budget_reservation() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let store = db.session_store();
    let session_id = SessionId(Uuid::new_v4());

    store
        .create_session(
            &session_id,
            None,
            Satoshis(300),
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .expect("create");

    let res = store
        .reserve_budget(&session_id, Satoshis(100), "res_1")
        .await
        .expect("reserve");

    assert_eq!(
        res,
        SessionReservationResult::Reserved {
            reservation_id: "res_1".to_string(),
            remaining_sats: Satoshis(200),
        }
    );

    let session = store.get_session(&session_id).await.unwrap().unwrap();
    assert_eq!(session.remaining_sats, Satoshis(200));
    assert_eq!(session.reserved_sats, Satoshis(100));
    assert_eq!(session.spent_sats, Satoshis(0));

    // Check ledger
    let entries = store.get_accounting_entries(&session_id).await.unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].1, "session_reservation");
    assert_eq!(entries[1].2, 100);
}

#[tokio::test]
async fn test_insufficient_budget_rejection() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let store = db.session_store();
    let session_id = SessionId(Uuid::new_v4());

    store
        .create_session(
            &session_id,
            None,
            Satoshis(50),
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .expect("create");

    let res = store
        .reserve_budget(&session_id, Satoshis(60), "res_over")
        .await
        .expect("reserve attempt");

    assert_eq!(
        res,
        SessionReservationResult::InsufficientBudget {
            requested: Satoshis(60),
            available: Satoshis(50),
        }
    );

    let session = store.get_session(&session_id).await.unwrap().unwrap();
    assert_eq!(session.remaining_sats, Satoshis(50));
    assert_eq!(session.reserved_sats, Satoshis(0));
}

#[tokio::test]
async fn test_expired_session_rejection() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let store = db.session_store();
    let session_id = SessionId(Uuid::new_v4());

    // Expired in past
    let past_expiry = chrono::Utc::now().timestamp() - 100;
    store
        .create_session(&session_id, None, Satoshis(100), past_expiry)
        .await
        .expect("create");

    let res = store
        .reserve_budget(&session_id, Satoshis(10), "res_exp")
        .await
        .expect("reserve on expired");

    assert!(matches!(res, SessionReservationResult::Expired { .. }));

    let session = store.get_session(&session_id).await.unwrap().unwrap();
    assert_eq!(session.status, SessionStatus::Expired);
}

#[tokio::test]
async fn test_concurrent_reservations_multi_connection() {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));
    let session_id = SessionId(Uuid::new_v4());
    let store = db.session_store();

    // 1000 sats total
    store
        .create_session(
            &session_id,
            None,
            Satoshis(1000),
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .expect("create");

    let mut handles = Vec::new();
    let num_tasks = 10;

    // 10 tasks each reserving 100 sats
    for i in 0..num_tasks {
        let db_clone = db.clone();
        let sid = session_id.clone();
        let res_id = format!("res_{}", i);

        handles.push(tokio::spawn(async move {
            let s = db_clone.session_store();
            s.reserve_budget(&sid, Satoshis(100), &res_id).await
        }));
    }

    let mut success_count = 0;
    for h in handles {
        let res = h.await.unwrap().expect("query");
        if matches!(res, SessionReservationResult::Reserved { .. }) {
            success_count += 1;
        }
    }

    assert_eq!(success_count, 10, "All 10 reservations should succeed");

    let session = store.get_session(&session_id).await.unwrap().unwrap();
    assert_eq!(session.remaining_sats, Satoshis(0));
    assert_eq!(session.reserved_sats, Satoshis(1000));
    assert_eq!(session.status, SessionStatus::Exhausted);
}

#[tokio::test]
async fn test_concurrent_requests_exceeding_available_budget() {
    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));
    let session_id = SessionId(Uuid::new_v4());
    let store = db.session_store();

    // Budget of only 500 sats
    store
        .create_session(
            &session_id,
            None,
            Satoshis(500),
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .expect("create");

    let mut handles = Vec::new();
    let num_tasks = 10;

    // 10 tasks each racing to reserve 100 sats (total requested: 1000 > 500)
    for i in 0..num_tasks {
        let db_clone = db.clone();
        let sid = session_id.clone();
        let res_id = format!("res_race_{}", i);

        handles.push(tokio::spawn(async move {
            let s = db_clone.session_store();
            s.reserve_budget(&sid, Satoshis(100), &res_id).await
        }));
    }

    let mut successes = 0;
    let mut rejections = 0;

    for h in handles {
        let res = h.await.unwrap().expect("query");
        match res {
            SessionReservationResult::Reserved { .. } => successes += 1,
            SessionReservationResult::InsufficientBudget { .. }
            | SessionReservationResult::Inactive {
                status: SessionStatus::Exhausted,
            } => rejections += 1,
            other => panic!("Unexpected result: {:?}", other),
        }
    }

    assert_eq!(successes, 5, "Exactly 5 tasks should reserve 100 sats");
    assert_eq!(rejections, 5, "Exactly 5 tasks should be rejected");

    let session = store.get_session(&session_id).await.unwrap().unwrap();
    assert_eq!(session.remaining_sats, Satoshis(0));
    assert_eq!(session.reserved_sats, Satoshis(500));
}

#[tokio::test]
async fn test_successful_reservation_finalization() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let store = db.session_store();
    let session_id = SessionId(Uuid::new_v4());

    store
        .create_session(
            &session_id,
            None,
            Satoshis(200),
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .expect("create");

    store
        .reserve_budget(&session_id, Satoshis(80), "res_fin")
        .await
        .expect("reserve");

    store
        .finalize_reservation("res_fin")
        .await
        .expect("finalize");

    let session = store.get_session(&session_id).await.unwrap().unwrap();
    assert_eq!(session.remaining_sats, Satoshis(120));
    assert_eq!(session.reserved_sats, Satoshis(0));
    assert_eq!(session.spent_sats, Satoshis(80));

    // Verify ledger contains funding, reservation, and debit
    let entries = store.get_accounting_entries(&session_id).await.unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].1, "session_funding");
    assert_eq!(entries[1].1, "session_reservation");
    assert_eq!(entries[2].1, "session_debit");
    assert_eq!(entries[2].2, 80);
}

#[tokio::test]
async fn test_confirmed_failure_release() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let store = db.session_store();
    let session_id = SessionId(Uuid::new_v4());

    store
        .create_session(
            &session_id,
            None,
            Satoshis(100),
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .expect("create");

    // Reserve 100 (exhausting budget)
    store
        .reserve_budget(&session_id, Satoshis(100), "res_fail")
        .await
        .expect("reserve");

    let session_exhausted = store.get_session(&session_id).await.unwrap().unwrap();
    assert_eq!(session_exhausted.status, SessionStatus::Exhausted);
    assert_eq!(session_exhausted.remaining_sats, Satoshis(0));

    // Release reservation
    store
        .release_reservation("res_fail", "pre_generation_failure")
        .await
        .expect("release");

    let session_restored = store.get_session(&session_id).await.unwrap().unwrap();
    assert_eq!(session_restored.status, SessionStatus::Active);
    assert_eq!(session_restored.remaining_sats, Satoshis(100));
    assert_eq!(session_restored.reserved_sats, Satoshis(0));
    assert_eq!(session_restored.spent_sats, Satoshis(0));

    // Check ledger entries
    let entries = store.get_accounting_entries(&session_id).await.unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[2].1, "session_reservation_release");
    assert_eq!(entries[2].2, 100);
}

#[tokio::test]
async fn test_duplicate_finalization_rejection() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let store = db.session_store();
    let session_id = SessionId(Uuid::new_v4());

    store
        .create_session(
            &session_id,
            None,
            Satoshis(100),
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .expect("create");
    store
        .reserve_budget(&session_id, Satoshis(50), "res_dup_fin")
        .await
        .expect("reserve");

    // First finalization succeeds
    store
        .finalize_reservation("res_dup_fin")
        .await
        .expect("first finalization");

    // Second finalization must fail
    let res2 = store.finalize_reservation("res_dup_fin").await;
    assert!(res2.is_err(), "Duplicate finalization must fail");
    assert!(res2.unwrap_err().to_string().contains("Cannot finalize"));
}

#[tokio::test]
async fn test_duplicate_release_rejection() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let store = db.session_store();
    let session_id = SessionId(Uuid::new_v4());

    store
        .create_session(
            &session_id,
            None,
            Satoshis(100),
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .expect("create");
    store
        .reserve_budget(&session_id, Satoshis(50), "res_dup_rel")
        .await
        .expect("reserve");

    // First release succeeds
    store
        .release_reservation("res_dup_rel", "reason 1")
        .await
        .expect("first release");

    // Second release must fail
    let res2 = store.release_reservation("res_dup_rel", "reason 2").await;
    assert!(res2.is_err(), "Duplicate release must fail");
    assert!(res2.unwrap_err().to_string().contains("Cannot release"));
}

#[tokio::test]
async fn test_recovery_of_definitely_undispatched_reservations() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let store = db.session_store();
    let session_id = SessionId(Uuid::new_v4());

    store
        .create_session(
            &session_id,
            None,
            Satoshis(500),
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .expect("create");

    store
        .reserve_budget(&session_id, Satoshis(100), "undispatched_1")
        .await
        .expect("reserve 1");
    store
        .reserve_budget(&session_id, Satoshis(150), "undispatched_2")
        .await
        .expect("reserve 2");

    let definitely_undispatched = vec!["undispatched_1".to_string(), "undispatched_2".to_string()];

    let recovered = store
        .recover_in_flight_reservations(&definitely_undispatched)
        .await
        .expect("recover");
    assert_eq!(recovered, 2);

    let session = store.get_session(&session_id).await.unwrap().unwrap();
    assert_eq!(session.remaining_sats, Satoshis(500));
    assert_eq!(session.reserved_sats, Satoshis(0));
}

#[tokio::test]
async fn test_retention_of_ambiguous_reservations() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let store = db.session_store();
    let session_id = SessionId(Uuid::new_v4());

    store
        .create_session(
            &session_id,
            None,
            Satoshis(500),
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .expect("create");

    // Ambiguous reservation (in flight during crash, outcome unknown)
    store
        .reserve_budget(&session_id, Satoshis(200), "ambiguous_req_1")
        .await
        .expect("reserve");

    // Run recovery with NO definitely undispatched list (it was dispatched to Ollama)
    let recovered = store
        .recover_in_flight_reservations(&[])
        .await
        .expect("recover");
    assert_eq!(recovered, 0);

    // Reservation MUST be retained in reserved_sats, NOT refunded!
    let session = store.get_session(&session_id).await.unwrap().unwrap();
    assert_eq!(session.remaining_sats, Satoshis(300));
    assert_eq!(
        session.reserved_sats,
        Satoshis(200),
        "Ambiguous reservation must be retained"
    );

    // Check reservation status in DB is reconciliation_required
    let status_str: String =
        sqlx::query_scalar("SELECT status FROM session_reservations WHERE id = 'ambiguous_req_1'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(status_str, "reconciliation_required");
}

#[tokio::test]
async fn test_session_macaroon_compatibility() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let store = db.session_store();
    let session_uuid = Uuid::new_v4();
    let session_id = SessionId(session_uuid);

    store
        .create_session(
            &session_id,
            None,
            Satoshis(500),
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .expect("create");

    let macaroon_service = MacaroonService::new(
        b"01234567890123456789012345678901".to_vec(),
        "satsgrid-node",
    );
    let payment_hash =
        PaymentHash("aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899".to_string());

    let caveats = vec![
        Caveat::Session(session_uuid.to_string()),
        Caveat::Budget(500),
        Caveat::Capability("inference".to_string()),
        Caveat::ExpiresAt(chrono::Utc::now().timestamp() as u64 + 3600),
    ];

    let macaroon = macaroon_service.mint(&payment_hash, caveats).unwrap();

    let (extracted_id, extracted_budget) = SessionBudgetManager::extract_session_caveats(&macaroon);
    assert_eq!(extracted_id, Some(session_id.clone()));
    assert_eq!(extracted_budget, Some(Satoshis(500)));

    // Manager with store debits correctly using extracted ID
    let manager = SessionBudgetManager::with_store(store);
    let remaining = manager
        .debit_session(&extracted_id.unwrap(), Satoshis(75))
        .await
        .expect("debit via extracted macaroon session");
    assert_eq!(remaining, Satoshis(425));
}

#[tokio::test]
async fn test_existing_session_api_regression() {
    use axum::http::StatusCode;
    use satsgrid::node::api::routes::create_routes;
    use satsgrid::node::api::AppState;
    use satsgrid::node::lightning::mock::MockLightningBackend;
    use satsgrid::node::proxy::openai::OpenAiProxy;
    use serde_json::json;
    use tower::ServiceExt;

    let dir = tempdir().expect("create tempdir");
    let db = Arc::new(Database::init(dir.path()).await.expect("init db"));

    let backend = Arc::new(MockLightningBackend::new());
    let macaroon_service = Arc::new(MacaroonService::new(
        b"01234567890123456789012345678901".to_vec(),
        "satsgrid-node",
    ));
    let budget_manager = Arc::new(SessionBudgetManager::with_store(db.session_store()));

    let config = satsgrid::config::schema::NodeConfig::default();
    let proxy = OpenAiProxy::new("http://127.0.0.1:11434");

    let state = AppState {
        config: Arc::new(config.clone()),
        live_pricing: Arc::new(tokio::sync::RwLock::new(config.pricing.clone())),
        lightning: backend.clone(),
        budget_manager: budget_manager.clone(),
        macaroon_service: macaroon_service.clone(),
        proxy,
        stats: Arc::new(satsgrid::node::api::NodeStats::default()),
        admin_token: Arc::new("admin123".to_string()),
        db: Some(db.clone()),
    };

    let app = create_routes(state);

    // Call POST /v1/session/new
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/session/new")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&json!({ "budget_sats": 350 })).unwrap(),
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PAYMENT_REQUIRED);

    let auth_header = resp
        .headers()
        .get("WWW-Authenticate")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(auth_header.starts_with("L402"));

    // Extract challenge to parse macaroon
    let challenge = satsgrid::node::gate::L402Challenge::from_header_value(auth_header)
        .expect("parse challenge");
    let macaroon = Macaroon::from_base64(&challenge.macaroon).expect("parse macaroon base64");
    let (extracted_id, extracted_budget) = SessionBudgetManager::extract_session_caveats(&macaroon);

    let session_id = extracted_id.expect("session caveat present");
    assert_eq!(extracted_budget, Some(Satoshis(350)));

    // Verify session was persisted in SQLite
    let record = db
        .session_store()
        .get_session(&session_id)
        .await
        .unwrap()
        .expect("session must exist in SQLite");
    assert_eq!(record.total_budget_sats, Satoshis(350));
    assert_eq!(record.remaining_sats, Satoshis(350));
    assert_eq!(record.status, SessionStatus::Active);
}
