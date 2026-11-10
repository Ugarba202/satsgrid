use crate::common::error::{Error, Result};
use sqlx::{Connection, Executor, Pool, Sqlite, SqliteConnection};

/// Versioned metadata table ensuring migration idempotency and trackability.
pub const MIGRATIONS_METADATA_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS _satsgrid_migrations (
    version INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    applied_at INTEGER NOT NULL
);
"#;

pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
    pub requires_foreign_keys_disabled: bool,
}

/// Ordered list of immutable database migrations.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "init_system_metadata",
        sql: r#"
        CREATE TABLE IF NOT EXISTS system_metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );
        INSERT OR IGNORE INTO system_metadata (key, value, updated_at)
        VALUES ('schema_version', '1', unixepoch());
        "#,
        requires_foreign_keys_disabled: false,
    },
    Migration {
        version: 2,
        name: "create_payment_authorizations",
        sql: r#"
        CREATE TABLE IF NOT EXISTS payment_authorizations (
            payment_hash TEXT PRIMARY KEY,
            auth_type TEXT NOT NULL,
            state TEXT NOT NULL,
            idempotency_key TEXT,
            request_id TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            reserved_at INTEGER NOT NULL,
            settled_at INTEGER,
            expires_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_payment_authorizations_state 
            ON payment_authorizations(state);

        CREATE INDEX IF NOT EXISTS idx_payment_authorizations_expires_at 
            ON payment_authorizations(expires_at);

        CREATE INDEX IF NOT EXISTS idx_payment_authorizations_idempotency 
            ON payment_authorizations(idempotency_key) 
            WHERE idempotency_key IS NOT NULL;
        "#,
        requires_foreign_keys_disabled: false,
    },
    Migration {
        version: 3,
        name: "create_sessions_and_reservations",
        sql: r#"
        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            payment_hash TEXT UNIQUE,
            total_budget_sats INTEGER NOT NULL CHECK (total_budget_sats >= 0),
            remaining_sats INTEGER NOT NULL CHECK (remaining_sats >= 0),
            reserved_sats INTEGER NOT NULL DEFAULT 0 CHECK (reserved_sats >= 0),
            spent_sats INTEGER NOT NULL DEFAULT 0 CHECK (spent_sats >= 0),
            status TEXT NOT NULL CHECK (status IN ('active', 'exhausted', 'expired', 'closed')),
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            CONSTRAINT chk_session_budget_sum CHECK (remaining_sats + reserved_sats + spent_sats = total_budget_sats)
        );

        CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status);
        CREATE INDEX IF NOT EXISTS idx_sessions_expires_at ON sessions(expires_at);

        CREATE TABLE IF NOT EXISTS session_reservations (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            reserved_sats INTEGER NOT NULL CHECK (reserved_sats > 0),
            status TEXT NOT NULL CHECK (status IN ('reserved', 'finalized', 'released', 'reconciliation_required')),
            created_at INTEGER NOT NULL,
            finalized_at INTEGER,
            failure_reason TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_session_reservations_session_id ON session_reservations(session_id);
        CREATE INDEX IF NOT EXISTS idx_session_reservations_status ON session_reservations(status);

        CREATE TABLE IF NOT EXISTS accounting_entries (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            entry_uuid TEXT NOT NULL UNIQUE,
            session_id TEXT REFERENCES sessions(id) ON DELETE CASCADE,
            reservation_id TEXT REFERENCES session_reservations(id) ON DELETE SET NULL,
            entry_type TEXT NOT NULL CHECK (entry_type IN ('session_funding', 'session_reservation', 'session_debit', 'session_reservation_release', 'recovery_adjustment')),
            amount_sats INTEGER NOT NULL CHECK (amount_sats >= 0),
            created_at INTEGER NOT NULL,
            description TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_accounting_entries_session_id ON accounting_entries(session_id);
        CREATE INDEX IF NOT EXISTS idx_accounting_entries_entry_uuid ON accounting_entries(entry_uuid);
        "#,
        requires_foreign_keys_disabled: false,
    },
    Migration {
        version: 4,
        name: "create_inference_requests",
        sql: r#"
        CREATE TABLE IF NOT EXISTS inference_requests (
            id TEXT PRIMARY KEY,
            payment_hash TEXT,
            session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL,
            reservation_id TEXT REFERENCES session_reservations(id) ON DELETE SET NULL,
            idempotency_key TEXT,
            model TEXT NOT NULL,
            request_mode TEXT NOT NULL CHECK (request_mode IN ('streaming', 'non_streaming')),
            status TEXT NOT NULL CHECK (status IN ('authorized', 'reserved', 'processing', 'completed', 'failed', 'stream_interrupted', 'reconciliation_required')),
            created_at INTEGER NOT NULL,
            reserved_at INTEGER NOT NULL,
            started_at INTEGER,
            completed_at INTEGER,
            failed_at INTEGER,
            actual_prompt_tokens INTEGER,
            actual_completion_tokens INTEGER,
            charged_sats INTEGER NOT NULL CHECK (charged_sats >= 0),
            error_code TEXT,
            error_category TEXT,
            reconciliation_required BOOLEAN NOT NULL DEFAULT 0,
            CONSTRAINT chk_auth_source CHECK (payment_hash IS NOT NULL OR session_id IS NOT NULL),
            FOREIGN KEY (payment_hash) REFERENCES payment_authorizations(payment_hash) ON DELETE RESTRICT
        );

        CREATE INDEX IF NOT EXISTS idx_inference_requests_status ON inference_requests(status);
        CREATE INDEX IF NOT EXISTS idx_inference_requests_payment_hash ON inference_requests(payment_hash);
        CREATE INDEX IF NOT EXISTS idx_inference_requests_session_id ON inference_requests(session_id);
        CREATE INDEX IF NOT EXISTS idx_inference_requests_reservation_id ON inference_requests(reservation_id);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_inference_requests_session_idempotency ON inference_requests(session_id, idempotency_key) WHERE session_id IS NOT NULL AND idempotency_key IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_inference_requests_idempotency ON inference_requests(idempotency_key) WHERE idempotency_key IS NOT NULL;
        "#,
        requires_foreign_keys_disabled: false,
    },
    Migration {
        version: 5,
        name: "add_session_idempotency_unique_index",
        sql: r#"
        CREATE UNIQUE INDEX IF NOT EXISTS idx_inference_requests_session_idempotency ON inference_requests(session_id, idempotency_key) WHERE session_id IS NOT NULL AND idempotency_key IS NOT NULL;
        "#,
        requires_foreign_keys_disabled: false,
    },
    Migration {
        version: 6,
        name: "support_pending_payment_sessions",
        sql: r#"
        -- 1. Create new sessions table with updated CHECK constraints supporting 'pending_payment'
        CREATE TABLE IF NOT EXISTS sessions_new (
            id TEXT PRIMARY KEY,
            payment_hash TEXT UNIQUE,
            total_budget_sats INTEGER NOT NULL CHECK (total_budget_sats >= 0),
            remaining_sats INTEGER NOT NULL CHECK (remaining_sats >= 0),
            reserved_sats INTEGER NOT NULL DEFAULT 0 CHECK (reserved_sats >= 0),
            spent_sats INTEGER NOT NULL DEFAULT 0 CHECK (spent_sats >= 0),
            status TEXT NOT NULL CHECK (status IN ('pending_payment', 'active', 'exhausted', 'expired', 'closed')),
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            CONSTRAINT chk_session_budget_sum CHECK (
                (status = 'pending_payment' AND remaining_sats = 0 AND reserved_sats = 0 AND spent_sats = 0)
                OR
                (status != 'pending_payment' AND remaining_sats + reserved_sats + spent_sats = total_budget_sats)
            )
        );

        -- 2. Migrate existing sessions data
        INSERT INTO sessions_new (
            id, payment_hash, total_budget_sats, remaining_sats,
            reserved_sats, spent_sats, status, created_at, expires_at, updated_at
        )
        SELECT 
            id, payment_hash, total_budget_sats, remaining_sats,
            reserved_sats, spent_sats, status, created_at, expires_at, updated_at
        FROM sessions;

        -- 3. Replace old table with new table
        DROP TABLE sessions;
        ALTER TABLE sessions_new RENAME TO sessions;

        -- 4. Recreate indices on sessions
        CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status);
        CREATE INDEX IF NOT EXISTS idx_sessions_expires_at ON sessions(expires_at);
        "#,
        requires_foreign_keys_disabled: true,
    },
    Migration {
        version: 7,
        name: "payguard_invoices_and_two_phase_auth",
        sql: r#"
        -- 1. Create durable invoices table for PayGuard
        CREATE TABLE IF NOT EXISTS invoices (
            payment_hash TEXT PRIMARY KEY,
            bolt11 TEXT NOT NULL,
            amount_sats INTEGER NOT NULL CHECK (amount_sats >= 0),
            purpose TEXT NOT NULL CHECK (purpose IN ('pay_per_request', 'session_funding')),
            session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL,
            status TEXT NOT NULL CHECK (status IN ('pending', 'settled', 'expired', 'cancelled')),
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            settled_at INTEGER,
            updated_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_invoices_status ON invoices(status);
        CREATE INDEX IF NOT EXISTS idx_invoices_purpose ON invoices(purpose);
        CREATE INDEX IF NOT EXISTS idx_invoices_session_id ON invoices(session_id) WHERE session_id IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_invoices_expires_at ON invoices(expires_at);

        -- 2. Add request fingerprint and release columns to payment_authorizations
        ALTER TABLE payment_authorizations ADD COLUMN request_fingerprint TEXT;
        ALTER TABLE payment_authorizations ADD COLUMN release_reason TEXT;
        ALTER TABLE payment_authorizations ADD COLUMN released_at INTEGER;

        CREATE INDEX IF NOT EXISTS idx_payment_authorizations_fingerprint
            ON payment_authorizations(request_fingerprint)
            WHERE request_fingerprint IS NOT NULL;
        "#,
        requires_foreign_keys_disabled: false,
    },
    Migration {
        version: 8,
        name: "session_closure_tracking_and_settlement",
        sql: r#"
        -- 1. Add closure metadata columns to sessions table
        ALTER TABLE sessions ADD COLUMN closed_at INTEGER;
        ALTER TABLE sessions ADD COLUMN closed_reason TEXT;

        CREATE INDEX IF NOT EXISTS idx_sessions_closed_at ON sessions(closed_at) WHERE closed_at IS NOT NULL;
        "#,
        requires_foreign_keys_disabled: false,
    },
    Migration {
        version: 9,
        name: "payguard_reconciliation_items",
        sql: r#"
        CREATE TABLE IF NOT EXISTS reconciliation_items (
            id TEXT PRIMARY KEY,
            item_type TEXT NOT NULL CHECK (item_type IN ('invoice', 'inference_request', 'session_late_funding')),
            reference_id TEXT NOT NULL,
            payment_hash TEXT,
            session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL,
            status TEXT NOT NULL CHECK (status IN ('unresolved', 'in_progress', 'resolved', 'ignored')),
            reason TEXT NOT NULL,
            error_details TEXT,
            attempt_count INTEGER NOT NULL DEFAULT 0,
            first_detected_at INTEGER NOT NULL,
            last_attempt_at INTEGER,
            next_retry_at INTEGER,
            resolved_at INTEGER
        );

        CREATE INDEX IF NOT EXISTS idx_reconciliation_status ON reconciliation_items(status);
        CREATE INDEX IF NOT EXISTS idx_reconciliation_next_retry ON reconciliation_items(next_retry_at) WHERE status = 'unresolved';
        CREATE INDEX IF NOT EXISTS idx_reconciliation_reference ON reconciliation_items(reference_id);
        "#,
        requires_foreign_keys_disabled: false,
    },
];

/// Execute pending migrations against the SQLite pool in an atomic transaction.
pub async fn run_migrations(pool: &Pool<Sqlite>) -> Result<()> {
    // 1. Acquire a dedicated connection for migrations
    let mut conn = pool.acquire().await.map_err(|e| {
        Error::Internal(format!(
            "Failed to acquire connection for migrations: {}",
            e
        ))
    })?;

    // 2. Ensure migrations tracking table exists
    conn.execute(MIGRATIONS_METADATA_TABLE).await.map_err(|e| {
        Error::Internal(format!("Failed to create migrations tracking table: {}", e))
    })?;

    // 3. Fetch applied versions
    let applied_versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _satsgrid_migrations ORDER BY version ASC")
            .fetch_all(&mut *conn)
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch applied migrations: {}", e)))?;

    // 4. Apply missing migrations sequentially
    for migration in MIGRATIONS {
        if applied_versions.contains(&migration.version) {
            continue;
        }

        tracing::info!(
            version = migration.version,
            name = migration.name,
            "Applying database migration"
        );

        if migration.requires_foreign_keys_disabled {
            run_migration_with_table_rebuild(&mut conn, migration).await?;
        } else {
            run_standard_migration(&mut conn, migration).await?;
        }
    }

    Ok(())
}

async fn run_standard_migration(conn: &mut SqliteConnection, migration: &Migration) -> Result<()> {
    let mut tx = conn
        .begin()
        .await
        .map_err(|e| Error::Internal(format!("Failed to begin migration transaction: {}", e)))?;

    for statement in migration.sql.split(';') {
        let trimmed = statement.trim();
        if trimmed.is_empty() {
            continue;
        }
        sqlx::query(trimmed).execute(&mut *tx).await.map_err(|e| {
            Error::Internal(format!(
                "Migration {} ({}) failed: {}",
                migration.version, migration.name, e
            ))
        })?;
    }

    sqlx::query(
        "INSERT INTO _satsgrid_migrations (version, name, applied_at) VALUES (?1, ?2, unixepoch())",
    )
    .bind(migration.version)
    .bind(migration.name)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        Error::Internal(format!(
            "Failed to record migration {}: {}",
            migration.version, e
        ))
    })?;

    tx.commit().await.map_err(|e| {
        Error::Internal(format!(
            "Failed to commit migration {}: {}",
            migration.version, e
        ))
    })?;

    Ok(())
}

pub async fn run_migration_with_table_rebuild(
    conn: &mut SqliteConnection,
    migration: &Migration,
) -> Result<()> {
    // 1. Disable foreign keys outside transaction
    sqlx::query("PRAGMA foreign_keys = OFF;")
        .execute(&mut *conn)
        .await
        .map_err(|e| Error::Internal(format!("Failed to disable foreign keys: {}", e)))?;

    // 2. Verify foreign keys are disabled
    let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys;")
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| Error::Internal(format!("Failed to query foreign_keys PRAGMA: {}", e)))?;

    if fk != 0 {
        // Restore foreign keys before failing
        let _ = sqlx::query("PRAGMA foreign_keys = ON;")
            .execute(&mut *conn)
            .await;
        return Err(Error::Internal(
            "Foreign keys could not be disabled on dedicated connection for table rebuild"
                .to_string(),
        ));
    }

    // 3. Begin transaction
    let migration_result: Result<()> = async {
        conn.execute("BEGIN IMMEDIATE;").await.map_err(|e| {
            Error::Internal(format!("Failed to begin transaction for table rebuild: {}", e))
        })?;

        // Execute migration DDL/DML statements
        for statement in migration.sql.split(';') {
            let trimmed = statement.trim();
            if trimmed.is_empty() {
                continue;
            }
            conn.execute(trimmed).await.map_err(|e| {
                Error::Internal(format!(
                    "Migration {} ({}) statement failed: {}",
                    migration.version, migration.name, e
                ))
            })?;
        }

        // 4. Run foreign_key_check before commit
        let fk_violations: Vec<(String, i64, String, i64)> =
            sqlx::query_as("PRAGMA foreign_key_check;")
                .fetch_all(&mut *conn)
                .await
                .map_err(|e| {
                    Error::Internal(format!("Failed to execute foreign_key_check: {}", e))
                })?;

        if !fk_violations.is_empty() {
            let _ = conn.execute("ROLLBACK;").await;
            return Err(Error::Internal(format!(
                "Foreign key integrity check failed after migration {} with {} violations",
                migration.version,
                fk_violations.len()
            )));
        }

        // 5. Record migration metadata
        sqlx::query(
            "INSERT INTO _satsgrid_migrations (version, name, applied_at) VALUES (?1, ?2, unixepoch())",
        )
        .bind(migration.version)
        .bind(migration.name)
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            Error::Internal(format!(
                "Failed to record migration {}: {}",
                migration.version, e
            ))
        })?;

        // 6. Commit transaction
        conn.execute("COMMIT;").await.map_err(|e| {
            Error::Internal(format!("Failed to commit table rebuild migration: {}", e))
        })?;

        Ok(())
    }
    .await;

    // In case of error inside transaction, attempt rollback if still inside
    if migration_result.is_err() {
        let _ = conn.execute("ROLLBACK;").await;
    }

    // 7. Re-enable foreign keys
    let reenable_res = sqlx::query("PRAGMA foreign_keys = ON;")
        .execute(&mut *conn)
        .await;

    // 8. Verify foreign keys are enabled
    let verify_res: Result<()> = match sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys;")
        .fetch_one(&mut *conn)
        .await
    {
        Ok(fk_val) => {
            if fk_val == 1 {
                Ok(())
            } else {
                Err(Error::Internal(
                    "Foreign keys could not be re-enabled after table rebuild migration"
                        .to_string(),
                ))
            }
        }
        Err(e) => Err(Error::Internal(format!(
            "Failed to verify foreign_keys after re-enabling: {}",
            e
        ))),
    };

    migration_result?;
    reenable_res
        .map_err(|e| Error::Internal(format!("Failed to re-enable foreign keys: {}", e)))?;
    verify_res?;

    Ok(())
}
