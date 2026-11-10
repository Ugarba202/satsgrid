use crate::common::error::{Error, Result};
use crate::common::types::{PaymentHash, SessionId};
use sqlx::{Pool, Sqlite};
use std::str::FromStr;
use uuid::Uuid;

/// Type of item requiring reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconciliationItemType {
    Invoice,
    InferenceRequest,
    SessionLateFunding,
}

impl ReconciliationItemType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Invoice => "invoice",
            Self::InferenceRequest => "inference_request",
            Self::SessionLateFunding => "session_late_funding",
        }
    }
}

impl FromStr for ReconciliationItemType {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "invoice" => Ok(Self::Invoice),
            "inference_request" => Ok(Self::InferenceRequest),
            "session_late_funding" => Ok(Self::SessionLateFunding),
            _ => Err(()),
        }
    }
}

/// Status of a reconciliation item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconciliationStatus {
    Unresolved,
    InProgress,
    Resolved,
    Ignored,
}

impl ReconciliationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unresolved => "unresolved",
            Self::InProgress => "in_progress",
            Self::Resolved => "resolved",
            Self::Ignored => "ignored",
        }
    }
}

impl FromStr for ReconciliationStatus {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "unresolved" => Ok(Self::Unresolved),
            "in_progress" => Ok(Self::InProgress),
            "resolved" => Ok(Self::Resolved),
            "ignored" => Ok(Self::Ignored),
            _ => Err(()),
        }
    }
}

/// Durable record of an item requiring operator or automatic reconciliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciliationItemRecord {
    pub id: String,
    pub item_type: ReconciliationItemType,
    pub reference_id: String,
    pub payment_hash: Option<PaymentHash>,
    pub session_id: Option<SessionId>,
    pub status: ReconciliationStatus,
    pub reason: String,
    pub error_details: Option<String>,
    pub attempt_count: i64,
    pub first_detected_at: i64,
    pub last_attempt_at: Option<i64>,
    pub next_retry_at: Option<i64>,
    pub resolved_at: Option<i64>,
}

type ReconRow = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    Option<String>,
    i64,
    i64,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);

/// Durable store for PayGuard reconciliation items.
#[derive(Clone, Debug)]
pub struct ReconciliationStore {
    pool: Pool<Sqlite>,
}

impl ReconciliationStore {
    pub fn new(pool: Pool<Sqlite>) -> Self {
        Self { pool }
    }

    /// Record a new reconciliation item or update an existing one if not already resolved.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_or_update_item(
        &self,
        item_type: ReconciliationItemType,
        reference_id: &str,
        payment_hash: Option<&PaymentHash>,
        session_id: Option<&SessionId>,
        reason: &str,
        error_details: Option<&str>,
        next_retry_at: Option<i64>,
    ) -> Result<String> {
        let now = chrono::Utc::now().timestamp();
        let query_existing = r#"
            SELECT id, status
            FROM reconciliation_items
            WHERE reference_id = ?1 AND status != 'resolved'
            ORDER BY first_detected_at DESC LIMIT 1
        "#;

        let existing: Option<(String, String)> = sqlx::query_as(query_existing)
            .bind(reference_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("Failed to query reconciliation item: {}", e)))?;

        if let Some((id, _status)) = existing {
            let update_query = r#"
                UPDATE reconciliation_items
                SET last_attempt_at = ?1,
                    attempt_count = attempt_count + 1,
                    reason = ?2,
                    error_details = ?3,
                    next_retry_at = ?4
                WHERE id = ?5
            "#;
            sqlx::query(update_query)
                .bind(now)
                .bind(reason)
                .bind(error_details)
                .bind(next_retry_at)
                .bind(&id)
                .execute(&self.pool)
                .await
                .map_err(|e| {
                    Error::Internal(format!("Failed to update reconciliation item: {}", e))
                })?;
            Ok(id)
        } else {
            let id = Uuid::new_v4().to_string();
            let insert_query = r#"
                INSERT INTO reconciliation_items (
                    id, item_type, reference_id, payment_hash, session_id,
                    status, reason, error_details, attempt_count,
                    first_detected_at, last_attempt_at, next_retry_at, resolved_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, 'unresolved', ?6, ?7, 0, ?8, NULL, ?9, NULL)
            "#;
            sqlx::query(insert_query)
                .bind(&id)
                .bind(item_type.as_str())
                .bind(reference_id)
                .bind(payment_hash.map(|h| h.0.as_str()))
                .bind(session_id.map(|s| s.0.to_string()))
                .bind(reason)
                .bind(error_details)
                .bind(now)
                .bind(next_retry_at)
                .execute(&self.pool)
                .await
                .map_err(|e| {
                    Error::Internal(format!("Failed to insert reconciliation item: {}", e))
                })?;
            Ok(id)
        }
    }

    /// Mark an item as resolved.
    pub async fn mark_resolved(&self, id: &str) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let query = r#"
            UPDATE reconciliation_items
            SET status = 'resolved',
                resolved_at = ?1
            WHERE id = ?2
        "#;
        sqlx::query(query)
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to resolve reconciliation item: {}", e))
            })?;
        Ok(())
    }

    /// Mark an item by reference as resolved.
    pub async fn mark_resolved_by_reference(&self, reference_id: &str) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let query = r#"
            UPDATE reconciliation_items
            SET status = 'resolved',
                resolved_at = ?1
            WHERE reference_id = ?2 AND status != 'resolved'
        "#;
        sqlx::query(query)
            .bind(now)
            .bind(reference_id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to resolve reconciliation item: {}", e))
            })?;
        Ok(())
    }

    /// Fetch items eligible for retry (`status = 'unresolved'` and `next_retry_at <= now` or `NULL`).
    pub async fn get_eligible_retry_items(
        &self,
        limit: i64,
    ) -> Result<Vec<ReconciliationItemRecord>> {
        let now = chrono::Utc::now().timestamp();
        let query = r#"
            SELECT id, item_type, reference_id, payment_hash, session_id,
                   status, reason, error_details, attempt_count,
                   first_detected_at, last_attempt_at, next_retry_at, resolved_at
            FROM reconciliation_items
            WHERE status = 'unresolved' AND (next_retry_at IS NULL OR next_retry_at <= ?1)
            ORDER BY first_detected_at ASC
            LIMIT ?2
        "#;

        let rows: Vec<ReconRow> = sqlx::query_as(query)
            .bind(now)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| {
                Error::Internal(format!(
                    "Failed to fetch eligible reconciliation items: {}",
                    e
                ))
            })?;

        let mut results = Vec::new();
        for r in rows {
            let item_type = ReconciliationItemType::from_str(&r.1).map_err(|_| {
                Error::Internal("Invalid reconciliation item_type in DB".to_string())
            })?;
            let status = ReconciliationStatus::from_str(&r.5)
                .map_err(|_| Error::Internal("Invalid reconciliation status in DB".to_string()))?;
            let session_id = match r.4 {
                Some(s) => Some(Uuid::parse_str(&s).map(SessionId).map_err(|e| {
                    Error::Internal(format!("Invalid UUID in reconciliation item: {}", e))
                })?),
                None => None,
            };

            results.push(ReconciliationItemRecord {
                id: r.0,
                item_type,
                reference_id: r.2,
                payment_hash: r.3.map(PaymentHash),
                session_id,
                status,
                reason: r.6,
                error_details: r.7,
                attempt_count: r.8,
                first_detected_at: r.9,
                last_attempt_at: r.10,
                next_retry_at: r.11,
                resolved_at: r.12,
            });
        }

        Ok(results)
    }

    /// Count total unresolved reconciliation items.
    pub async fn count_unresolved(&self) -> Result<i64> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM reconciliation_items WHERE status = 'unresolved'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("Failed to count unresolved items: {}", e)))?;

        Ok(count)
    }
}
