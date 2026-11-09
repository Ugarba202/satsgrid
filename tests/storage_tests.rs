use satsgrid::node::storage::Database;
use tempfile::tempdir;

#[tokio::test]
async fn test_database_initialization_in_tempdir() {
    let dir = tempdir().expect("create tempdir");
    let db_path = dir.path().join("test_state.db");

    let db = Database::init(&db_path)
        .await
        .expect("Database initialization should succeed");

    assert!(db_path.exists(), "database file should exist on disk");
    assert!(db.ping().await.is_ok(), "database ping should succeed");
}

#[tokio::test]
async fn test_database_parent_directory_creation() {
    let dir = tempdir().expect("create tempdir");
    let nested_path = dir.path().join("nested").join("sub").join("state.db");

    let db = Database::init(&nested_path)
        .await
        .expect("Database should create parent directories");

    assert!(nested_path.exists());
    assert!(db.ping().await.is_ok());
}

#[tokio::test]
async fn test_in_memory_database_initialization() {
    let db = Database::init_in_memory()
        .await
        .expect("In-memory database should initialize");

    assert_eq!(db.db_path().to_str().unwrap(), ":memory:");
    assert!(db.ping().await.is_ok());
}

#[tokio::test]
async fn test_migration_execution_and_system_metadata() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path())
        .await
        .expect("Database init should run migrations");

    let version_val: String =
        sqlx::query_scalar("SELECT value FROM system_metadata WHERE key = 'schema_version'")
            .fetch_one(db.pool())
            .await
            .expect("schema_version metadata should exist");

    assert_eq!(version_val, "1");

    let applied_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _satsgrid_migrations")
        .fetch_one(db.pool())
        .await
        .expect("count migrations");

    assert!(applied_count >= 2);
}

#[tokio::test]
async fn test_repeated_migration_initialization_idempotency() {
    let dir = tempdir().expect("create tempdir");
    let db_path = dir.path().join("state.db");

    // First initialization
    let db1 = Database::init(&db_path).await.expect("first init");
    drop(db1);

    // Second initialization on existing file (must not fail or duplicate migrations)
    let db2 = Database::init(&db_path).await.expect("second init");
    let applied_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _satsgrid_migrations")
        .fetch_one(db2.pool())
        .await
        .expect("count migrations");

    assert!(applied_count >= 2);
}

#[tokio::test]
async fn test_wal_journal_mode_configured() {
    let dir = tempdir().expect("create tempdir");
    let db_path = dir.path().join("wal_test.db");

    let db = Database::init(&db_path).await.expect("init");

    let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(db.pool())
        .await
        .expect("fetch journal_mode");

    assert_eq!(mode.to_lowercase(), "wal");
}

#[tokio::test]
async fn test_foreign_keys_pragmas_enforced() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init");

    let fk: i32 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(db.pool())
        .await
        .expect("fetch foreign_keys");

    assert_eq!(fk, 1, "Foreign keys must be actively enabled");
}

#[tokio::test]
async fn test_busy_timeout_configured() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init");

    let timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
        .fetch_one(db.pool())
        .await
        .expect("fetch busy_timeout");

    assert_eq!(timeout, 5000, "Busy timeout must be configured to 5000ms");
}

#[tokio::test]
async fn test_existing_data_preserved_across_reconnect() {
    let dir = tempdir().expect("create tempdir");
    let db_path = dir.path().join("persistent.db");

    {
        let db1 = Database::init(&db_path).await.expect("init db1");
        sqlx::query("INSERT INTO system_metadata (key, value, updated_at) VALUES ('custom_test_key', 'stored_value_123', 1000)")
            .execute(db1.pool())
            .await
            .expect("insert data");
    }

    {
        let db2 = Database::init(&db_path).await.expect("init db2");
        let val: String =
            sqlx::query_scalar("SELECT value FROM system_metadata WHERE key = 'custom_test_key'")
                .fetch_one(db2.pool())
                .await
                .expect("fetch data");
        assert_eq!(val, "stored_value_123");
    }
}

#[tokio::test]
async fn test_invalid_database_path_produces_clear_error() {
    // Attempting to open an unwritable path should produce a clean Error::Config or Error::Internal
    let invalid_path = "/nonexistent_root_dir_for_test/state.db";
    let res = Database::init(invalid_path).await;
    assert!(res.is_err(), "Invalid root path should fail to initialize");
    let err_str = res.unwrap_err().to_string();
    assert!(
        err_str.contains("Failed to create database directory")
            || err_str.contains("Failed to open SQLite")
    );
}

#[tokio::test]
async fn test_migration_2_payment_authorizations_schema() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _satsgrid_migrations")
        .fetch_one(db.pool())
        .await
        .expect("count migrations");

    assert!(count >= 2, "Migrations 1 and 2 (or more) should be applied");

    // Verify payment_authorizations table exists
    let table_exists: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table' AND name='payment_authorizations'",
    )
    .fetch_optional(db.pool())
    .await
    .expect("check table existence");

    assert_eq!(table_exists, Some("payment_authorizations".to_string()));
}

#[tokio::test]
async fn test_migration_3_sessions_and_reservations_schema() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");

    for table in &["sessions", "session_reservations", "accounting_entries"] {
        let exists: Option<String> =
            sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='table' AND name=?1")
                .bind(table)
                .fetch_optional(db.pool())
                .await
                .expect("check table existence");

        assert_eq!(
            exists,
            Some(table.to_string()),
            "Table '{}' should exist",
            table
        );
    }
}

#[tokio::test]
async fn test_migration_4_inference_requests_schema() {
    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");

    let exists: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table' AND name='inference_requests'",
    )
    .fetch_optional(db.pool())
    .await
    .expect("check table existence");

    assert_eq!(exists, Some("inference_requests".to_string()));
}

#[tokio::test]
async fn test_migration_6_preserves_historical_records_and_foreign_keys() {
    use satsgrid::node::storage::migrations::{run_migrations, MIGRATIONS};
    use sqlx::sqlite::{
        SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
    };
    use std::str::FromStr;
    use std::time::Duration;

    let dir = tempdir().expect("create tempdir");
    let db_path = dir.path().join("v5_upgrade.db");
    let uri = format!("sqlite://{}?mode=rwc", db_path.to_str().unwrap());

    let opts = SqliteConnectOptions::from_str(&uri)
        .unwrap()
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_millis(5000));

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await
        .unwrap();

    // 1. Manually setup metadata table and run migrations 1 through 5
    sqlx::query(satsgrid::node::storage::migrations::MIGRATIONS_METADATA_TABLE)
        .execute(&pool)
        .await
        .unwrap();

    for migration in &MIGRATIONS[0..5] {
        let mut tx = pool.begin().await.unwrap();
        for statement in migration.sql.split(';') {
            let trimmed = statement.trim();
            if !trimmed.is_empty() {
                sqlx::query(trimmed).execute(&mut *tx).await.unwrap();
            }
        }
        sqlx::query("INSERT INTO _satsgrid_migrations (version, name, applied_at) VALUES (?1, ?2, unixepoch())")
            .bind(migration.version)
            .bind(migration.name)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    // 2. Insert historical data at version 5
    let session_id = "test-session-v5-001";
    let payment_hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let reservation_id = "test-res-v5-001";
    let request_id = "test-req-v5-001";
    let entry_uuid = "test-entry-uuid-001";

    // Insert payment auth
    sqlx::query("INSERT INTO payment_authorizations (payment_hash, auth_type, state, idempotency_key, request_id, created_at, reserved_at, settled_at, expires_at) VALUES (?1, 'pay_per_request', 'consumed', NULL, ?2, 1000, 1000, 1005, 2000)")
        .bind(payment_hash)
        .bind(request_id)
        .execute(&pool)
        .await
        .unwrap();

    // Insert session (active, 100 budget, 40 remaining, 30 reserved, 30 spent)
    sqlx::query("INSERT INTO sessions (id, payment_hash, total_budget_sats, remaining_sats, reserved_sats, spent_sats, status, created_at, expires_at, updated_at) VALUES (?1, ?2, 100, 40, 30, 30, 'active', 1000, 5000, 1000)")
        .bind(session_id)
        .bind(payment_hash)
        .execute(&pool)
        .await
        .unwrap();

    // Insert reservation
    sqlx::query("INSERT INTO session_reservations (id, session_id, reserved_sats, status, created_at, finalized_at, failure_reason) VALUES (?1, ?2, 30, 'reserved', 1000, NULL, NULL)")
        .bind(reservation_id)
        .bind(session_id)
        .execute(&pool)
        .await
        .unwrap();

    // Insert accounting entry (referencing sessions and session_reservations)
    sqlx::query("INSERT INTO accounting_entries (entry_uuid, session_id, reservation_id, entry_type, amount_sats, created_at, description) VALUES (?1, ?2, ?3, 'session_reservation', 30, 1000, 'reservation 1')")
        .bind(entry_uuid)
        .bind(session_id)
        .bind(reservation_id)
        .execute(&pool)
        .await
        .unwrap();

    // Insert inference request (referencing sessions, session_reservations, and payment_authorizations)
    sqlx::query("INSERT INTO inference_requests (id, payment_hash, session_id, reservation_id, idempotency_key, model, request_mode, status, created_at, reserved_at, started_at, completed_at, failed_at, actual_prompt_tokens, actual_completion_tokens, charged_sats, error_code, error_category, reconciliation_required) VALUES (?1, ?2, ?3, ?4, 'idem-req-1', 'llama3.2', 'non_streaming', 'processing', 1000, 1000, 1001, NULL, NULL, 10, 20, 30, NULL, NULL, 0)")
        .bind(request_id)
        .bind(payment_hash)
        .bind(session_id)
        .bind(reservation_id)
        .execute(&pool)
        .await
        .unwrap();

    // 3. Run migration 6 via run_migrations
    run_migrations(&pool)
        .await
        .expect("Migration 6 must succeed");

    // Test A & C: Verify all dependent and parent records are preserved intact!
    let session_row: (String, Option<String>, i64, i64, i64, i64, String) = sqlx::query_as(
        "SELECT id, payment_hash, total_budget_sats, remaining_sats, reserved_sats, spent_sats, status FROM sessions WHERE id = ?1"
    )
    .bind(session_id)
    .fetch_one(&pool)
    .await
    .expect("Historical session must be preserved");

    assert_eq!(session_row.0, session_id);
    assert_eq!(session_row.1.as_deref(), Some(payment_hash));
    assert_eq!(session_row.2, 100);
    assert_eq!(session_row.3, 40);
    assert_eq!(session_row.4, 30);
    assert_eq!(session_row.5, 30);
    assert_eq!(session_row.6, "active");

    let res_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM session_reservations WHERE id = ?1 AND session_id = ?2",
    )
    .bind(reservation_id)
    .bind(session_id)
    .fetch_one(&pool)
    .await
    .expect("query session_reservations");
    assert_eq!(
        res_count, 1,
        "Reservation record MUST NOT be cascade-deleted"
    );

    let entry_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM accounting_entries WHERE entry_uuid = ?1 AND session_id = ?2",
    )
    .bind(entry_uuid)
    .bind(session_id)
    .fetch_one(&pool)
    .await
    .expect("query accounting_entries");
    assert_eq!(
        entry_count, 1,
        "Accounting entry record MUST NOT be cascade-deleted"
    );

    let inf_req: (Option<String>, Option<String>) =
        sqlx::query_as("SELECT session_id, reservation_id FROM inference_requests WHERE id = ?1")
            .bind(request_id)
            .fetch_one(&pool)
            .await
            .expect("query inference_requests");
    assert_eq!(
        inf_req.0.as_deref(),
        Some(session_id),
        "Inference request session_id MUST NOT be set to NULL"
    );
    assert_eq!(
        inf_req.1.as_deref(),
        Some(reservation_id),
        "Inference request reservation_id MUST NOT be set to NULL"
    );

    // Test B: Foreign-key integrity check and foreign keys actively enabled
    let violations: Vec<(String, i64, String, i64)> = sqlx::query_as("PRAGMA foreign_key_check;")
        .fetch_all(&pool)
        .await
        .expect("run foreign_key_check");
    assert!(
        violations.is_empty(),
        "foreign_key_check must return 0 violations"
    );

    let fk_enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys;")
        .fetch_one(&pool)
        .await
        .expect("check foreign_keys");
    assert_eq!(
        fk_enabled, 1,
        "foreign_keys must remain enabled on connection pool"
    );

    // Test D: Schema preservation (status indices, constraints, payment_hash uniqueness)
    let idx_status: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='index' AND name='idx_sessions_status'",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert!(idx_status.is_some(), "idx_sessions_status must be present");

    let idx_expires: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='index' AND name='idx_sessions_expires_at'",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert!(
        idx_expires.is_some(),
        "idx_sessions_expires_at must be present"
    );

    // Unique payment hash test
    let dup_insert = sqlx::query("INSERT INTO sessions (id, payment_hash, total_budget_sats, remaining_sats, reserved_sats, spent_sats, status, created_at, expires_at, updated_at) VALUES ('dup-sess', ?1, 100, 100, 0, 0, 'active', 2000, 6000, 2000)")
        .bind(payment_hash)
        .execute(&pool)
        .await;
    assert!(
        dup_insert.is_err(),
        "Duplicate payment_hash must violate UNIQUE constraint"
    );

    // Check pending_payment constraint allows 0,0,0
    let pending_insert = sqlx::query("INSERT INTO sessions (id, payment_hash, total_budget_sats, remaining_sats, reserved_sats, spent_sats, status, created_at, expires_at, updated_at) VALUES ('pending-sess', 'phash-pending-test', 200, 0, 0, 0, 'pending_payment', 2000, 6000, 2000)")
        .execute(&pool)
        .await;
    assert!(
        pending_insert.is_ok(),
        "pending_payment with 0 remaining, 0 reserved, 0 spent must be valid"
    );

    // Test E: Migration idempotency (calling run_migrations again is a no-op)
    run_migrations(&pool)
        .await
        .expect("Idempotent run_migrations must succeed");
    let v6_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM _satsgrid_migrations WHERE version = 6")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(v6_count, 1, "Migration 6 recorded exactly once");
}

#[tokio::test]
async fn test_migration_failure_safety_and_rollback() {
    use satsgrid::node::storage::migrations::{run_migration_with_table_rebuild, Migration};
    use sqlx::sqlite::{
        SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
    };
    use std::str::FromStr;
    use std::time::Duration;

    let dir = tempdir().expect("create tempdir");
    let db_path = dir.path().join("failure_safety.db");
    let uri = format!("sqlite://{}?mode=rwc", db_path.to_str().unwrap());

    let opts = SqliteConnectOptions::from_str(&uri)
        .unwrap()
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_millis(5000));

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await
        .unwrap();

    let mut conn = pool.acquire().await.unwrap();

    // Setup base tables
    sqlx::query("CREATE TABLE parents (id TEXT PRIMARY KEY);")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE children (id TEXT PRIMARY KEY, parent_id TEXT REFERENCES parents(id) ON DELETE CASCADE);").execute(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO parents (id) VALUES ('p1');")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("INSERT INTO children (id, parent_id) VALUES ('c1', 'p1');")
        .execute(&mut *conn)
        .await
        .unwrap();

    // Failing migration: introduces a foreign key violation or bad sql
    let failing_migration = Migration {
        version: 99,
        name: "failing_rebuild",
        sql: r#"
        CREATE TABLE parents_new (id TEXT PRIMARY KEY);
        -- Intentionally do not copy 'p1', leaving children referencing missing 'p1'
        DROP TABLE parents;
        ALTER TABLE parents_new RENAME TO parents;
        "#,
        requires_foreign_keys_disabled: true,
    };

    let result = run_migration_with_table_rebuild(&mut conn, &failing_migration).await;
    assert!(
        result.is_err(),
        "Migration with foreign_key_check violations must fail"
    );

    // Verify foreign keys are restored to ON on the connection
    let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys;")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        fk, 1,
        "Foreign keys must be restored to ON even when migration fails"
    );

    // Verify rollback restored original tables and data
    let parent_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM parents WHERE id = 'p1'")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        parent_count, 1,
        "Parent table and data must be intact after rollback"
    );

    let child_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM children WHERE id = 'c1'")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        child_count, 1,
        "Child table and data must be intact after rollback"
    );
}

#[tokio::test]
async fn test_migration_7_invoices_schema_and_integrity() {
    use satsgrid::common::types::{PaymentHash, Satoshis, SessionId};
    use satsgrid::node::storage::invoice::{InvoicePurpose, InvoiceStatus};
    use uuid::Uuid;

    let dir = tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let pool = db.pool();

    // 1. Verify migration 7 is recorded in _satsgrid_migrations
    let v7_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM _satsgrid_migrations WHERE version = 7")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(v7_count, 1, "Migration 7 must be recorded");

    // 2. Verify invoices table exists and columns are queryable
    let inv_store = db.invoice_store();
    let payment_hash =
        PaymentHash("1111222233334444555566667777888899990000aaaabbbbccccddddeeeeffff".to_string());
    let bolt11 = "lnbc100u1testinvoice".to_string();
    let amount = Satoshis(100);
    let expires_at = chrono::Utc::now().timestamp() + 3600;

    inv_store
        .create_invoice(
            &payment_hash,
            &bolt11,
            amount,
            InvoicePurpose::PayPerRequest,
            None,
            expires_at,
        )
        .await
        .expect("create invoice must succeed");

    // Retrieve invoice
    let record = inv_store
        .get_invoice(&payment_hash)
        .await
        .expect("get invoice query")
        .expect("invoice record must exist");

    assert_eq!(record.payment_hash, payment_hash);
    assert_eq!(record.bolt11, bolt11);
    assert_eq!(record.amount_sats, amount);
    assert_eq!(record.purpose, InvoicePurpose::PayPerRequest);
    assert_eq!(record.session_id, None);
    assert_eq!(record.status, InvoiceStatus::Pending);
    assert_eq!(record.settled_at, None);

    // 3. Test settlement update
    let settled_time = chrono::Utc::now().timestamp();
    let res = inv_store
        .mark_settled(&payment_hash, settled_time)
        .await
        .expect("mark settled");
    assert_eq!(
        res,
        satsgrid::node::storage::InvoiceSettlementResult::Settled
    );

    let settled_record = inv_store.get_invoice(&payment_hash).await.unwrap().unwrap();
    assert_eq!(settled_record.status, InvoiceStatus::Settled);
    assert_eq!(settled_record.settled_at, Some(settled_time));

    // Test idempotent re-settlement
    let res_again = inv_store
        .mark_settled(&payment_hash, settled_time + 10)
        .await
        .expect("idempotent mark settled");
    assert_eq!(
        res_again,
        satsgrid::node::storage::InvoiceSettlementResult::AlreadySettled
    );

    // Test settlement conflict on expired invoice
    let exp_hash =
        PaymentHash("exp-test-hash-1234567890abcdef1234567890abcdef1234567890abcdef".to_string());
    inv_store
        .create_invoice(
            &exp_hash,
            "bolt11_exp",
            Satoshis(50),
            InvoicePurpose::PayPerRequest,
            None,
            chrono::Utc::now().timestamp() - 10,
        )
        .await
        .unwrap();
    let marked_exp = inv_store.mark_expired(&exp_hash).await.unwrap();
    assert!(marked_exp, "Invoice should be marked expired");

    let conflict_res = inv_store
        .mark_settled(&exp_hash, chrono::Utc::now().timestamp())
        .await
        .unwrap();
    assert_eq!(
        conflict_res,
        satsgrid::node::storage::InvoiceSettlementResult::ConflictExpired
    );

    // Stored status must remain expired, not overwritten
    let exp_record = inv_store.get_invoice(&exp_hash).await.unwrap().unwrap();
    assert_eq!(exp_record.status, InvoiceStatus::Expired);
    assert_eq!(exp_record.settled_at, None);

    // Test settlement on non-existent invoice
    let missing_hash =
        PaymentHash("missing-hash-000000000000000000000000000000000000000000000000".to_string());
    let missing_res = inv_store
        .mark_settled(&missing_hash, chrono::Utc::now().timestamp())
        .await
        .unwrap();
    assert_eq!(
        missing_res,
        satsgrid::node::storage::InvoiceSettlementResult::NotFound
    );

    // 4. Test duplicate payment_hash rejection (UNIQUE constraint)
    let dup_res = inv_store
        .create_invoice(
            &payment_hash,
            "another_bolt11",
            Satoshis(200),
            InvoicePurpose::PayPerRequest,
            None,
            expires_at,
        )
        .await;
    assert!(dup_res.is_err(), "Duplicate payment hash must be rejected");

    // 5. Test session_id foreign key association
    let sess_store = db.session_store();
    let session_id = SessionId(Uuid::new_v4());
    let sess_hash =
        PaymentHash("9999888877776666555544443333222211110000aaaabbbbccccddddeeeeffff".to_string());
    sess_store
        .create_pending_session(&session_id, Some(&sess_hash), Satoshis(500), expires_at)
        .await
        .expect("create pending session");

    inv_store
        .create_invoice(
            &sess_hash,
            "bolt11_for_session",
            Satoshis(500),
            InvoicePurpose::SessionFunding,
            Some(&session_id),
            expires_at,
        )
        .await
        .expect("create session invoice");

    let sess_inv = inv_store.get_invoice(&sess_hash).await.unwrap().unwrap();
    assert_eq!(sess_inv.session_id, Some(session_id));
    assert_eq!(sess_inv.purpose, InvoicePurpose::SessionFunding);

    // 6. Test payment_authorizations schema extension (request_fingerprint, release_reason, released_at)
    let pa_insert = sqlx::query(
        "INSERT INTO payment_authorizations (
            payment_hash, auth_type, state, idempotency_key, request_id, created_at, reserved_at, settled_at, expires_at,
            request_fingerprint, release_reason, released_at
        ) VALUES ('pa-test-hash', 'pay_per_request', 'released', NULL, 'req-1', 1000, 1000, NULL, 5000, 'fp-sha256-test', 'connect_failure', 1010)"
    ).execute(pool).await;
    assert!(
        pa_insert.is_ok(),
        "payment_authorizations table must accept extended Migration 7 columns"
    );
}
