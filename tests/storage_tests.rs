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
