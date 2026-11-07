use crate::common::error::{Error, Result};
use crate::node::storage::migrations::run_migrations;
use crate::node::storage::payment::PaymentStore;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Pool, Sqlite};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

pub const DEFAULT_DB_FILENAME: &str = "state.db";

/// Central persistent storage handle wrapping the SQLite connection pool.
#[derive(Clone)]
pub struct Database {
    pool: Pool<Sqlite>,
    db_path: PathBuf,
}

impl fmt::Debug for Database {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Database")
            .field("db_path", &self.db_path)
            .finish()
    }
}

impl Database {
    /// Initialize a new Database from a data directory or explicit database file path.
    ///
    /// If `path_or_dir` points to a directory (or filename is omitted), `state.db` is appended.
    /// Safely creates parent directories if they do not exist.
    pub async fn init(path_or_dir: impl AsRef<Path>) -> Result<Self> {
        let raw_path = path_or_dir.as_ref();
        let db_path = if raw_path.is_dir() || raw_path.extension().is_none() {
            raw_path.join(DEFAULT_DB_FILENAME)
        } else {
            raw_path.to_path_buf()
        };

        if let Some(parent) = db_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| {
                    Error::Config(format!(
                        "Failed to create database directory '{}': {}",
                        parent.display(),
                        e
                    ))
                })?;
            }
        }

        let db_str = db_path.to_str().ok_or_else(|| {
            Error::Config("Database path contains invalid UTF-8 characters".to_string())
        })?;

        let connection_uri = format!("sqlite://{}?mode=rwc", db_str);

        let connect_opts = SqliteConnectOptions::from_str(&connection_uri)
            .map_err(|e| Error::Config(format!("Invalid SQLite connection options: {}", e)))?
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_millis(5000));

        let pool = SqlitePoolOptions::new()
            .max_connections(10)
            .min_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .idle_timeout(Duration::from_secs(30))
            .connect_with(connect_opts)
            .await
            .map_err(|e| {
                Error::Internal(format!(
                    "Failed to open SQLite database at '{}': {}",
                    db_path.display(),
                    e
                ))
            })?;

        // Verify connection and SQLite PRAGMAs
        Self::verify_pragmas(&pool).await?;

        // Execute pending migrations
        run_migrations(&pool).await?;

        tracing::info!(
            path = %db_path.display(),
            "SQLite persistent storage initialized with WAL mode"
        );

        Ok(Self { pool, db_path })
    }

    /// Open an ephemeral in-memory database strictly for testing.
    pub async fn init_in_memory() -> Result<Self> {
        let connect_opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .map_err(|e| Error::Config(format!("Invalid in-memory SQLite URI: {}", e)))?
            .foreign_keys(true)
            .busy_timeout(Duration::from_millis(5000));

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(connect_opts)
            .await
            .map_err(|e| Error::Internal(format!("Failed to open in-memory SQLite: {}", e)))?;

        run_migrations(&pool).await?;

        Ok(Self {
            pool,
            db_path: PathBuf::from(":memory:"),
        })
    }

    /// Access the underlying connection pool.
    pub fn pool(&self) -> &Pool<Sqlite> {
        &self.pool
    }

    /// Get the resolved database file path.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Get a `PaymentStore` handle for payment authorization operations.
    pub fn payment_store(&self) -> PaymentStore {
        PaymentStore::new(self.pool.clone())
    }

    /// Get a `SessionStore` handle for persistent session budget operations.
    pub fn session_store(&self) -> crate::node::storage::session::SessionStore {
        crate::node::storage::session::SessionStore::new(self.pool.clone())
    }

    /// Get a `RequestStore` handle for inference request lifecycle operations.
    pub fn request_store(&self) -> crate::node::storage::request::RequestStore {
        crate::node::storage::request::RequestStore::new(self.pool.clone())
    }

    /// Atomically finalize an inference request and its associated session reservation (if any)
    /// in a single SQLite transaction.
    ///
    /// Ensures that an inference request cannot be marked completed without its session reservation
    /// being finalized simultaneously, preventing split-brain inconsistencies between request logs
    /// and the accounting ledger.
    pub async fn finalize_request_and_session(
        &self,
        request_id: &str,
        session_reservation_id: Option<&str>,
        actual_prompt_tokens: Option<i64>,
        actual_completion_tokens: Option<i64>,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let mut conn = self.pool.acquire().await.map_err(|e| {
            Error::Internal(format!(
                "Failed to acquire DB connection for atomic finalization: {}",
                e
            ))
        })?;

        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Internal(format!("Failed to begin transaction: {}", e)))?;

        // 1. If a session reservation is associated, finalize it
        if let Some(res_id) = session_reservation_id {
            // Fetch reservation details
            let fetch_res = r#"
                SELECT session_id, reserved_sats, status
                FROM session_reservations
                WHERE id = ?1
            "#;

            let row: Option<(String, i64, String)> = sqlx::query_as(fetch_res)
                .bind(res_id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(|e| {
                    Error::Internal(format!(
                        "Failed to fetch reservation in atomic finalization: {}",
                        e
                    ))
                })?;

            let (session_id_str, reserved_sats, status_str) = match row {
                Some(r) => r,
                None => {
                    let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                    return Err(Error::Internal(format!(
                        "Reservation '{}' not found during atomic finalization",
                        res_id
                    )));
                }
            };

            if status_str != "reserved" {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                return Err(Error::Internal(format!(
                    "Cannot finalize reservation '{}': current status is '{}'",
                    res_id, status_str
                )));
            }

            // Update reservation status to finalized
            let update_res = sqlx::query(
                "UPDATE session_reservations SET status = 'finalized', finalized_at = ?1 WHERE id = ?2 AND status = 'reserved'",
            )
            .bind(now)
            .bind(res_id)
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Internal(format!("Failed to finalize reservation record: {}", e)))?;

            if update_res.rows_affected() == 0 {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                return Err(Error::Internal(format!(
                    "Concurrent modification during finalization of reservation '{}'",
                    res_id
                )));
            }

            // Update session balance: decrement reserved, increment spent
            let update_session = r#"
                UPDATE sessions
                SET reserved_sats = reserved_sats - ?1,
                    spent_sats = spent_sats + ?1,
                    updated_at = ?2
                WHERE id = ?3 AND reserved_sats >= ?1
            "#;

            sqlx::query(update_session)
                .bind(reserved_sats)
                .bind(now)
                .bind(&session_id_str)
                .execute(&mut *conn)
                .await
                .map_err(|e| {
                    Error::Internal(format!("Failed to update session spent balance: {}", e))
                })?;

            // Record accounting event: session_debit
            let entry_uuid = uuid::Uuid::new_v4().to_string();
            let ledger_query = r#"
                INSERT INTO accounting_entries (
                    entry_uuid, session_id, reservation_id, entry_type, amount_sats, created_at, description
                ) VALUES (?1, ?2, ?3, 'session_debit', ?4, ?5, ?6)
            "#;

            let desc = format!(
                "Finalized debit of {} sats for reservation {}",
                reserved_sats, res_id
            );
            sqlx::query(ledger_query)
                .bind(&entry_uuid)
                .bind(&session_id_str)
                .bind(res_id)
                .bind(reserved_sats)
                .bind(now)
                .bind(desc)
                .execute(&mut *conn)
                .await
                .map_err(|e| {
                    Error::Internal(format!("Failed to record debit ledger entry: {}", e))
                })?;
        }

        // 2. Mark inference request completed
        let query_req = r#"
            UPDATE inference_requests
            SET status = 'completed',
                completed_at = ?1,
                actual_prompt_tokens = ?2,
                actual_completion_tokens = ?3
            WHERE id = ?4 AND status IN ('processing', 'stream_interrupted')
        "#;

        let res_req = sqlx::query(query_req)
            .bind(now)
            .bind(actual_prompt_tokens)
            .bind(actual_completion_tokens)
            .bind(request_id)
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Internal(format!("Failed to mark request completed: {}", e)))?;

        if res_req.rows_affected() == 0 {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            return Err(Error::Internal(format!(
                "Cannot mark request '{}' completed: not in processing or stream_interrupted state",
                request_id
            )));
        }

        sqlx::query("COMMIT")
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Internal(format!("Failed to commit atomic finalization: {}", e)))?;

        Ok(())
    }

    /// Verify that essential PRAGMAs (foreign keys, WAL mode) are actively enforced.
    async fn verify_pragmas(pool: &Pool<Sqlite>) -> Result<()> {
        let fk: i32 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(pool)
            .await
            .map_err(|e| Error::Internal(format!("Failed to verify foreign_keys PRAGMA: {}", e)))?;

        if fk != 1 {
            return Err(Error::Internal(
                "SQLite foreign_keys PRAGMA is not enabled".to_string(),
            ));
        }

        let jm: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(pool)
            .await
            .map_err(|e| Error::Internal(format!("Failed to verify journal_mode PRAGMA: {}", e)))?;

        if !jm.eq_ignore_ascii_case("wal") && !jm.eq_ignore_ascii_case("memory") {
            tracing::warn!(
                journal_mode = %jm,
                "SQLite journal_mode is not WAL (expected for non-memory databases)"
            );
        }

        Ok(())
    }

    /// Health check query executing a trivial SELECT 1 to verify database responsiveness.
    pub async fn ping(&self) -> Result<()> {
        let _: i32 = sqlx::query_scalar("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("Database health ping failed: {}", e)))?;
        Ok(())
    }
}
