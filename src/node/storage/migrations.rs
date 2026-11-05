use crate::common::error::{Error, Result};
use sqlx::{Pool, Sqlite};

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
    },
    Migration {
        version: 5,
        name: "add_session_idempotency_unique_index",
        sql: r#"
        CREATE UNIQUE INDEX IF NOT EXISTS idx_inference_requests_session_idempotency ON inference_requests(session_id, idempotency_key) WHERE session_id IS NOT NULL AND idempotency_key IS NOT NULL;
        "#,
    },
];

/// Execute pending migrations against the SQLite pool in an atomic transaction.
pub async fn run_migrations(pool: &Pool<Sqlite>) -> Result<()> {
    // 1. Ensure migrations tracking table exists
    sqlx::query(MIGRATIONS_METADATA_TABLE)
        .execute(pool)
        .await
        .map_err(|e| {
            Error::Internal(format!("Failed to create migrations tracking table: {}", e))
        })?;

    // 2. Fetch applied versions
    let applied_versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _satsgrid_migrations ORDER BY version ASC")
            .fetch_all(pool)
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch applied migrations: {}", e)))?;

    // 3. Apply missing migrations sequentially
    for migration in MIGRATIONS {
        if applied_versions.contains(&migration.version) {
            continue;
        }

        tracing::info!(
            version = migration.version,
            name = migration.name,
            "Applying database migration"
        );

        let mut tx = pool.begin().await.map_err(|e| {
            Error::Internal(format!("Failed to begin migration transaction: {}", e))
        })?;

        // Execute DDL batch
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

        // Record successful migration
        sqlx::query("INSERT INTO _satsgrid_migrations (version, name, applied_at) VALUES (?1, ?2, unixepoch())")
            .bind(migration.version)
            .bind(migration.name)
            .execute(&mut *tx)
            .await
            .map_err(|e| Error::Internal(format!("Failed to record migration {}: {}", migration.version, e)))?;

        tx.commit().await.map_err(|e| {
            Error::Internal(format!(
                "Failed to commit migration {}: {}",
                migration.version, e
            ))
        })?;
    }

    Ok(())
}
