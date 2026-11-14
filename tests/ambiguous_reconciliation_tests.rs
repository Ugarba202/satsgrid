use satsgrid::common::types::{Satoshis, SessionId};
use satsgrid::node::storage::{Database, RequestMode, RequestStatus};
use tempfile::TempDir;
use uuid::Uuid;

async fn processing_session_request() -> (TempDir, Database, SessionId, String) {
    let dir = tempfile::tempdir().expect("create isolated database directory");
    let db = Database::init(dir.path())
        .await
        .expect("initialize database");
    let session_id = SessionId(Uuid::new_v4());
    let reservation_id = "ambiguous-reservation";
    let request_id = "ambiguous-request".to_string();

    let sessions = db.session_store();
    sessions
        .create_session(
            &session_id,
            None,
            Satoshis(100),
            chrono::Utc::now().timestamp() + 3600,
        )
        .await
        .expect("create test session");
    sessions
        .reserve_budget(&session_id, Satoshis(10), reservation_id)
        .await
        .expect("reserve test budget");

    let requests = db.request_store();
    requests
        .create_request(
            &request_id,
            None,
            Some(&session_id),
            Some(reservation_id),
            None,
            "test-model",
            RequestMode::Streaming,
            Satoshis(10),
        )
        .await
        .expect("create test inference request");
    requests
        .mark_processing(&request_id)
        .await
        .expect("mark request processing");

    (dir, db, session_id, request_id)
}

#[tokio::test]
async fn atomic_ambiguous_transition_protects_request_item_and_reservation() {
    let (_dir, db, session_id, request_id) = processing_session_request().await;
    let reason = "client_abort_or_timeout_after_dispatch";
    let details = "Client disconnected after upstream dispatch; execution outcome is unknown";

    let item_id = db
        .request_store()
        .mark_ambiguous_reconciliation_required(&request_id, reason, details)
        .await
        .expect("atomic transition");

    let request = db
        .request_store()
        .get_request(&request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request.status, RequestStatus::ReconciliationRequired);
    assert!(request.reconciliation_required);
    assert_eq!(request.error_code.as_deref(), Some(reason));

    let reservation_status: String = sqlx::query_scalar(
        "SELECT status FROM session_reservations WHERE id = 'ambiguous-reservation'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(reservation_status, "reconciliation_required");

    let session = db
        .session_store()
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session.remaining_sats, Satoshis(90));
    assert_eq!(session.reserved_sats, Satoshis(10));
    assert_eq!(session.spent_sats, Satoshis(0));

    let item = db
        .reconciliation_store()
        .get_item(&item_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.reference_id, request_id);
    assert_eq!(item.session_id, Some(session_id));
    assert_eq!(item.status.as_str(), "unresolved");
    assert_eq!(item.reason, reason);
    assert_eq!(item.error_details.as_deref(), Some(details));
    assert_eq!(item.attempt_count, 0);
    assert_eq!(item.last_attempt_at, None);
}

#[tokio::test]
async fn ambiguous_transition_rolls_back_request_and_reservation_on_storage_failure() {
    let (_dir, db, session_id, request_id) = processing_session_request().await;
    sqlx::query("DROP TABLE reconciliation_items")
        .execute(db.pool())
        .await
        .expect("induce isolated reconciliation storage failure");

    let result = db
        .request_store()
        .mark_ambiguous_reconciliation_required(
            &request_id,
            "client_abort_or_timeout_after_dispatch",
            "Unknown upstream outcome",
        )
        .await;
    assert!(result.is_err());

    let request = db
        .request_store()
        .get_request(&request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request.status, RequestStatus::Processing);
    assert!(!request.reconciliation_required);

    let reservation_status: String = sqlx::query_scalar(
        "SELECT status FROM session_reservations WHERE id = 'ambiguous-reservation'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(reservation_status, "reserved");
    let session = db
        .session_store()
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session.remaining_sats, Satoshis(90));
    assert_eq!(session.reserved_sats, Satoshis(10));
    assert_eq!(session.spent_sats, Satoshis(0));
}

#[tokio::test]
async fn repeated_ambiguous_transition_reuses_one_unresolved_item() {
    let (_dir, db, _session_id, request_id) = processing_session_request().await;
    let requests = db.request_store();

    let first = requests
        .mark_ambiguous_reconciliation_required(&request_id, "ambiguous_dispatch", "unknown")
        .await
        .unwrap();
    let second = requests
        .mark_ambiguous_reconciliation_required(&request_id, "ambiguous_dispatch", "unknown")
        .await
        .unwrap();

    assert_eq!(first, second);
    let item = db
        .reconciliation_store()
        .get_item(&first)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.attempt_count, 0);
    assert_eq!(item.last_attempt_at, None);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reconciliation_items WHERE item_type = 'inference_request' AND reference_id = ?1 AND status = 'unresolved'",
    )
    .bind(&request_id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn startup_recovery_preserves_retry_history_and_recovers_in_progress_item() {
    let (_dir, db, _session_id, request_id) = processing_session_request().await;
    let requests = db.request_store();
    let item_id = requests
        .mark_ambiguous_reconciliation_required(&request_id, "ambiguous_dispatch", "unknown")
        .await
        .unwrap();
    let last_attempt_at = 1_700_000_000_i64;
    sqlx::query(
        "UPDATE reconciliation_items SET status = 'in_progress', attempt_count = 4, last_attempt_at = ?1, next_retry_at = ?2 WHERE id = ?3",
    )
    .bind(last_attempt_at)
    .bind(last_attempt_at + 300)
    .bind(&item_id)
    .execute(db.pool())
    .await
    .unwrap();

    requests.recover_in_flight_requests().await.unwrap();

    let item = db
        .reconciliation_store()
        .get_item(&item_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.status.as_str(), "unresolved");
    assert_eq!(item.attempt_count, 4);
    assert_eq!(item.last_attempt_at, Some(last_attempt_at));
    assert_eq!(item.next_retry_at, None);
}

#[tokio::test]
async fn startup_repairs_missing_item_for_already_ambiguous_request() {
    let (_dir, db, session_id, request_id) = processing_session_request().await;
    let old_item_id = db
        .request_store()
        .mark_ambiguous_reconciliation_required(
            &request_id,
            "client_abort_or_timeout_after_dispatch",
            "original safe details",
        )
        .await
        .unwrap();
    sqlx::query("DELETE FROM reconciliation_items WHERE id = ?1")
        .bind(old_item_id)
        .execute(db.pool())
        .await
        .unwrap();

    let undispatched = db
        .request_store()
        .recover_in_flight_requests()
        .await
        .expect("startup recovery repairs reconciliation metadata");
    assert!(undispatched.is_empty());

    let items = db
        .reconciliation_store()
        .list_items(None, 10)
        .await
        .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].reference_id, request_id);
    assert_eq!(items[0].session_id, Some(session_id));
    assert_eq!(items[0].reason, "client_abort_or_timeout_after_dispatch");
    assert_eq!(items[0].status.as_str(), "unresolved");

    let reservation_status: String = sqlx::query_scalar(
        "SELECT status FROM session_reservations WHERE id = 'ambiguous-reservation'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(reservation_status, "reconciliation_required");
}

#[tokio::test]
async fn startup_does_not_duplicate_existing_unresolved_item() {
    let (_dir, db, _session_id, request_id) = processing_session_request().await;
    let requests = db.request_store();

    requests.recover_in_flight_requests().await.unwrap();
    requests.recover_in_flight_requests().await.unwrap();

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reconciliation_items WHERE item_type = 'inference_request' AND reference_id = ?1 AND status = 'unresolved'",
    )
    .bind(&request_id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(count, 1);

    let reservation_status: String = sqlx::query_scalar(
        "SELECT status FROM session_reservations WHERE id = 'ambiguous-reservation'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(reservation_status, "reconciliation_required");
}
