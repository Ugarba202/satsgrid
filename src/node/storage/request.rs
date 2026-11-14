use crate::common::error::{Error, Result};
use crate::common::types::{PaymentHash, Satoshis, SessionId};
use sqlx::{Pool, Sqlite};
use std::str::FromStr;

/// Request execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestMode {
    Streaming,
    NonStreaming,
}

impl RequestMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Streaming => "streaming",
            Self::NonStreaming => "non_streaming",
        }
    }
}

impl FromStr for RequestMode {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "streaming" => Ok(Self::Streaming),
            "non_streaming" => Ok(Self::NonStreaming),
            _ => Err(()),
        }
    }
}

/// Lifecycle status of an inference request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestStatus {
    Authorized,
    Reserved,
    Processing,
    Completed,
    Failed,
    StreamInterrupted,
    ReconciliationRequired,
}

impl RequestStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Authorized => "authorized",
            Self::Reserved => "reserved",
            Self::Processing => "processing",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::StreamInterrupted => "stream_interrupted",
            Self::ReconciliationRequired => "reconciliation_required",
        }
    }

    /// Check whether a transition from `self` to `target` is valid.
    pub fn can_transition_to(&self, target: RequestStatus) -> bool {
        match (self, target) {
            // Authorized -> Reserved
            (Self::Authorized, Self::Reserved) => true,
            // Reserved -> Processing (dispatch boundary)
            (Self::Reserved, Self::Processing) => true,
            // Reserved -> Failed (pre-dispatch rejection / aborted)
            (Self::Reserved, Self::Failed) => true,
            // Processing -> Completed (successful upstream response)
            (Self::Processing, Self::Completed) => true,
            // Processing -> Failed (upstream error / 500 / timeout)
            (Self::Processing, Self::Failed) => true,
            // Processing -> StreamInterrupted (client disconnect or socket broken mid-stream)
            (Self::Processing, Self::StreamInterrupted) => true,
            // Processing -> ReconciliationRequired (server crash recovery)
            (Self::Processing, Self::ReconciliationRequired) => true,
            // StreamInterrupted -> ReconciliationRequired
            (Self::StreamInterrupted, Self::ReconciliationRequired) => true,
            // StreamInterrupted -> Completed (if settled for partial tokens)
            (Self::StreamInterrupted, Self::Completed) => true,
            // Terminal states or illegal transitions
            _ => false,
        }
    }
}

impl FromStr for RequestStatus {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "authorized" => Ok(Self::Authorized),
            "reserved" => Ok(Self::Reserved),
            "processing" => Ok(Self::Processing),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "stream_interrupted" => Ok(Self::StreamInterrupted),
            "reconciliation_required" => Ok(Self::ReconciliationRequired),
            _ => Err(()),
        }
    }
}

use sqlx::Row;

/// Record representing a durable inference request in SQLite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferenceRequestRecord {
    pub id: String,
    pub payment_hash: Option<PaymentHash>,
    pub session_id: Option<SessionId>,
    pub reservation_id: Option<String>,
    pub idempotency_key: Option<String>,
    pub model: String,
    pub request_mode: RequestMode,
    pub status: RequestStatus,
    pub created_at: i64,
    pub reserved_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub failed_at: Option<i64>,
    pub actual_prompt_tokens: Option<i64>,
    pub actual_completion_tokens: Option<i64>,
    pub charged_sats: Satoshis,
    pub error_code: Option<String>,
    pub error_category: Option<String>,
    pub reconciliation_required: bool,
}

/// Durable store for inference request lifecycle management.
#[derive(Clone, Debug)]
pub struct RequestStore {
    pool: Pool<Sqlite>,
}

impl RequestStore {
    pub fn new(pool: Pool<Sqlite>) -> Self {
        Self { pool }
    }

    /// Create and persist an inference request record in the `reserved` status.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_request(
        &self,
        id: &str,
        payment_hash: Option<&PaymentHash>,
        session_id: Option<&SessionId>,
        reservation_id: Option<&str>,
        idempotency_key: Option<&str>,
        model: &str,
        request_mode: RequestMode,
        charged_sats: Satoshis,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let query = r#"
            INSERT INTO inference_requests (
                id, payment_hash, session_id, reservation_id, idempotency_key,
                model, request_mode, status, created_at, reserved_at, started_at,
                completed_at, failed_at, actual_prompt_tokens, actual_completion_tokens,
                charged_sats, error_code, error_category, reconciliation_required
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, 'reserved', ?8, ?8, NULL,
                NULL, NULL, NULL, NULL, ?9, NULL, NULL, 0
            )
        "#;

        let insert_res = sqlx::query(query)
            .bind(id)
            .bind(payment_hash.map(|h| h.0.as_str()))
            .bind(session_id.map(|s| s.0.to_string()))
            .bind(reservation_id)
            .bind(idempotency_key)
            .bind(model)
            .bind(request_mode.as_str())
            .bind(now)
            .bind(charged_sats.0 as i64)
            .execute(&self.pool)
            .await;

        match insert_res {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(db_err))
                if idempotency_key.is_some() && db_err.code().as_deref() == Some("2067") =>
            {
                Err(Error::VerificationFailed(
                    "A request with this idempotency key is currently in flight".to_string(),
                ))
            }
            Err(e) => Err(Error::Internal(format!(
                "Failed to create inference request: {}",
                e
            ))),
        }
    }

    /// Fetch an inference request by payment hash.
    pub async fn get_request_by_payment_hash(
        &self,
        payment_hash: &PaymentHash,
    ) -> Result<Option<InferenceRequestRecord>> {
        let query = r#"
            SELECT id
            FROM inference_requests
            WHERE payment_hash = ?1
        "#;

        let req_id: Option<String> = sqlx::query_scalar(query)
            .bind(&payment_hash.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| {
                Error::Internal(format!(
                    "Failed to fetch inference request by payment hash: {}",
                    e
                ))
            })?;

        match req_id {
            Some(id) => self.get_request(&id).await,
            None => Ok(None),
        }
    }

    /// Fetch an inference request by ID.
    pub async fn get_request(&self, id: &str) -> Result<Option<InferenceRequestRecord>> {
        let query = r#"
            SELECT id, payment_hash, session_id, reservation_id, idempotency_key,
                   model, request_mode, status, created_at, reserved_at, started_at,
                   completed_at, failed_at, actual_prompt_tokens, actual_completion_tokens,
                   charged_sats, error_code, error_category, reconciliation_required
            FROM inference_requests
            WHERE id = ?1
        "#;

        let row = sqlx::query(query)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch inference request: {}", e)))?;

        match row {
            Some(r) => {
                let req_id: String = r.get("id");
                let p_hash: Option<String> = r.get("payment_hash");
                let s_id: Option<String> = r.get("session_id");
                let res_id: Option<String> = r.get("reservation_id");
                let idem_key: Option<String> = r.get("idempotency_key");
                let model: String = r.get("model");
                let mode_str: String = r.get("request_mode");
                let status_str: String = r.get("status");
                let created_at: i64 = r.get("created_at");
                let reserved_at: i64 = r.get("reserved_at");
                let started_at: Option<i64> = r.get("started_at");
                let completed_at: Option<i64> = r.get("completed_at");
                let failed_at: Option<i64> = r.get("failed_at");
                let prompt_tokens: Option<i64> = r.get("actual_prompt_tokens");
                let completion_tokens: Option<i64> = r.get("actual_completion_tokens");
                let charged_sats: i64 = r.get("charged_sats");
                let err_code: Option<String> = r.get("error_code");
                let err_cat: Option<String> = r.get("error_category");
                let recon_req: bool = r.get("reconciliation_required");

                let payment_hash = p_hash.map(PaymentHash);
                let session_id = s_id.and_then(|s| uuid::Uuid::parse_str(&s).ok().map(SessionId));
                let request_mode = RequestMode::from_str(&mode_str)
                    .map_err(|_| Error::Internal("Invalid request mode in DB".to_string()))?;
                let status = RequestStatus::from_str(&status_str)
                    .map_err(|_| Error::Internal("Invalid request status in DB".to_string()))?;

                Ok(Some(InferenceRequestRecord {
                    id: req_id,
                    payment_hash,
                    session_id,
                    reservation_id: res_id,
                    idempotency_key: idem_key,
                    model,
                    request_mode,
                    status,
                    created_at,
                    reserved_at,
                    started_at,
                    completed_at,
                    failed_at,
                    actual_prompt_tokens: prompt_tokens,
                    actual_completion_tokens: completion_tokens,
                    charged_sats: Satoshis(charged_sats as u64),
                    error_code: err_code,
                    error_category: err_cat,
                    reconciliation_required: recon_req,
                }))
            }
            None => Ok(None),
        }
    }

    /// Check if a request with the given idempotency key already exists for a session.
    pub async fn find_by_idempotency_key(
        &self,
        session_id: &SessionId,
        idempotency_key: &str,
    ) -> Result<Option<InferenceRequestRecord>> {
        let query = r#"
            SELECT id FROM inference_requests
            WHERE session_id = ?1 AND idempotency_key = ?2
        "#;

        let req_id: Option<String> = sqlx::query_scalar(query)
            .bind(session_id.0.to_string())
            .bind(idempotency_key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("Failed to query idempotency key: {}", e)))?;

        match req_id {
            Some(id) => self.get_request(&id).await,
            None => Ok(None),
        }
    }

    /// Mark request as `processing` immediately before upstream dispatch (Durable Dispatch Boundary).
    ///
    /// Persists `started_at` timestamp. Strictly requires current state to be `reserved`.
    pub async fn mark_processing(&self, id: &str) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let query = r#"
            UPDATE inference_requests
            SET status = 'processing',
                started_at = ?1
            WHERE id = ?2 AND status = 'reserved'
        "#;

        let res = sqlx::query(query)
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("Failed to mark request processing: {}", e)))?;

        if res.rows_affected() == 0 {
            return Err(Error::Internal(format!(
                "Cannot mark request '{}' processing: not in reserved state",
                id
            )));
        }

        Ok(())
    }

    /// Mark request as `completed` upon successful upstream inference execution.
    ///
    /// Strictly requires current state to be `processing` or `stream_interrupted`.
    pub async fn mark_completed(
        &self,
        id: &str,
        prompt_tokens: Option<i64>,
        completion_tokens: Option<i64>,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let query = r#"
            UPDATE inference_requests
            SET status = 'completed',
                completed_at = ?1,
                actual_prompt_tokens = ?2,
                actual_completion_tokens = ?3
            WHERE id = ?4 AND status IN ('processing', 'stream_interrupted')
        "#;

        let res = sqlx::query(query)
            .bind(now)
            .bind(prompt_tokens)
            .bind(completion_tokens)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("Failed to mark request completed: {}", e)))?;

        if res.rows_affected() == 0 {
            return Err(Error::Internal(format!(
                "Cannot mark request '{}' completed: not in processing or stream_interrupted state",
                id
            )));
        }

        Ok(())
    }

    /// Mark request as `failed` (e.g. upstream error, timeout, or pre-dispatch abort).
    ///
    /// Strictly requires current state to be `reserved` or `processing`.
    pub async fn mark_failed(
        &self,
        id: &str,
        error_code: &str,
        error_category: &str,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let query = r#"
            UPDATE inference_requests
            SET status = 'failed',
                failed_at = ?1,
                error_code = ?2,
                error_category = ?3
            WHERE id = ?4 AND status IN ('reserved', 'processing')
        "#;

        let res = sqlx::query(query)
            .bind(now)
            .bind(error_code)
            .bind(error_category)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("Failed to mark request failed: {}", e)))?;

        if res.rows_affected() == 0 {
            return Err(Error::Internal(format!(
                "Cannot mark request '{}' failed: not in reserved or processing state",
                id
            )));
        }

        Ok(())
    }

    /// Mark request as `stream_interrupted` when an SSE stream is broken before final completion.
    ///
    /// Strictly requires current state to be `processing`.
    pub async fn mark_stream_interrupted(
        &self,
        id: &str,
        error_code: &str,
        completion_tokens: Option<i64>,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let query = r#"
            UPDATE inference_requests
            SET status = 'stream_interrupted',
                failed_at = ?1,
                error_code = ?2,
                error_category = 'stream_interrupted',
                actual_completion_tokens = ?3
            WHERE id = ?4 AND status = 'processing'
        "#;

        let res = sqlx::query(query)
            .bind(now)
            .bind(error_code)
            .bind(completion_tokens)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("Failed to mark stream interrupted: {}", e)))?;

        if res.rows_affected() == 0 {
            return Err(Error::Internal(format!(
                "Cannot mark request '{}' stream_interrupted: not in processing state",
                id
            )));
        }

        Ok(())
    }

    /// Mark request as requiring manual or operator reconciliation.
    pub async fn mark_reconciliation_required(&self, id: &str, reason: &str) -> Result<()> {
        let query = r#"
            UPDATE inference_requests
            SET status = 'reconciliation_required',
                reconciliation_required = 1,
                error_code = ?1,
                error_category = 'reconciliation_required'
            WHERE id = ?2 AND status IN ('reserved', 'processing', 'stream_interrupted')
        "#;

        let res = sqlx::query(query)
            .bind(reason)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                Error::Internal(format!(
                    "Failed to mark request reconciliation required: {}",
                    e
                ))
            })?;

        if res.rows_affected() == 0 {
            return Err(Error::Internal(format!(
                "Cannot mark request '{}' reconciliation required: invalid current state",
                id
            )));
        }

        Ok(())
    }

    /// Atomically protect an ambiguous request, its reservation, and its reconciliation item.
    /// Repeated calls update the same unresolved item and do not alter session balances.
    pub async fn mark_ambiguous_reconciliation_required(
        &self,
        request_id: &str,
        reason: &str,
        error_details: &str,
    ) -> Result<String> {
        self.mark_ambiguous_reconciliation_required_inner(request_id, reason, error_details, true)
            .await
    }

    /// Persist a completed stream's failed accounting/receipt finalization for manual recovery.
    /// The reconciliation item intentionally omits its payment hash so the invoice worker cannot
    /// resolve this request merely by observing the already-known Lightning settlement.
    pub async fn mark_stream_finalization_recovery_required(
        &self,
        request_id: &str,
        reason: &str,
        error_details: &str,
    ) -> Result<String> {
        self.mark_ambiguous_reconciliation_required_inner(request_id, reason, error_details, false)
            .await
    }

    async fn mark_ambiguous_reconciliation_required_inner(
        &self,
        request_id: &str,
        reason: &str,
        error_details: &str,
        attach_payment_hash: bool,
    ) -> Result<String> {
        let now = chrono::Utc::now().timestamp();
        let mut conn = self.pool.acquire().await.map_err(|e| {
            Error::Internal(format!(
                "Failed to acquire request reconciliation connection: {}",
                e
            ))
        })?;
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                Error::Internal(format!(
                    "Failed to begin ambiguous request transaction: {}",
                    e
                ))
            })?;

        let transition_result = async {
            let request: Option<(Option<String>, Option<String>)> = sqlx::query_as(
                "SELECT payment_hash, session_id FROM inference_requests WHERE id = ?1 AND status IN ('reserved', 'processing', 'stream_interrupted', 'reconciliation_required')",
            )
            .bind(request_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| Error::Internal(format!("Failed to load ambiguous inference request: {}", e)))?;
            let (payment_hash, session_id) = request.ok_or_else(|| {
                Error::Internal(format!(
                    "Cannot mark request '{}' reconciliation required: invalid current state or missing request",
                    request_id
                ))
            })?;

            let updated_request = sqlx::query(
                "UPDATE inference_requests SET status = 'reconciliation_required', reconciliation_required = 1, error_code = ?1, error_category = 'reconciliation_required' WHERE id = ?2 AND status IN ('reserved', 'processing', 'stream_interrupted', 'reconciliation_required')",
            )
            .bind(reason)
            .bind(request_id)
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Internal(format!("Failed to mark inference request reconciliation required: {}", e)))?;
            if updated_request.rows_affected() != 1 {
                return Err(Error::Internal(format!(
                    "Inference request '{}' changed during reconciliation transition",
                    request_id
                )));
            }

            let reservation_id: Option<String> = sqlx::query_scalar(
                "SELECT reservation_id FROM inference_requests WHERE id = ?1",
            )
            .bind(request_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(|e| Error::Internal(format!("Failed to load inference reservation: {}", e)))?;
            if let Some(reservation_id) = reservation_id {
                let updated_reservation = sqlx::query(
                    "UPDATE session_reservations SET status = 'reconciliation_required', failure_reason = ?1, finalized_at = ?2 WHERE id = ?3 AND session_id IS ?4 AND status IN ('reserved', 'reconciliation_required')",
                )
                .bind(reason)
                .bind(now)
                .bind(&reservation_id)
                .bind(session_id.as_deref())
                .execute(&mut *conn)
                .await
                .map_err(|e| Error::Internal(format!("Failed to protect ambiguous session reservation: {}", e)))?;
                if updated_reservation.rows_affected() != 1 {
                    return Err(Error::Internal(format!(
                        "Reservation '{}' is missing, mismatched, or not protectable",
                        reservation_id
                    )));
                }
            }

            let existing_item: Option<String> = sqlx::query_scalar(
                "SELECT id FROM reconciliation_items WHERE item_type = 'inference_request' AND reference_id = ?1 AND status IN ('unresolved', 'in_progress') ORDER BY first_detected_at DESC LIMIT 1",
            )
            .bind(request_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| Error::Internal(format!("Failed to query inference reconciliation item: {}", e)))?;

            if let Some(item_id) = existing_item {
                sqlx::query(
                    "UPDATE reconciliation_items SET payment_hash = CASE WHEN ?1 THEN COALESCE(?2, payment_hash) ELSE NULL END, session_id = COALESCE(?3, session_id), status = 'unresolved', reason = ?4, error_details = ?5, next_retry_at = NULL, resolved_at = NULL WHERE id = ?6",
                )
                .bind(attach_payment_hash)
                .bind(if attach_payment_hash { payment_hash.as_deref() } else { None })
                .bind(session_id.as_deref())
                .bind(reason)
                .bind(error_details)
                .bind(&item_id)
                .execute(&mut *conn)
                .await
                .map_err(|e| Error::Internal(format!("Failed to update inference reconciliation item: {}", e)))?;
                Ok(item_id)
            } else {
                let item_id = uuid::Uuid::new_v4().to_string();
                sqlx::query(
                    "INSERT INTO reconciliation_items (id, item_type, reference_id, payment_hash, session_id, status, reason, error_details, attempt_count, first_detected_at, last_attempt_at, next_retry_at, resolved_at) VALUES (?1, 'inference_request', ?2, ?3, ?4, 'unresolved', ?5, ?6, 0, ?7, NULL, NULL, NULL)",
                )
                .bind(&item_id)
                .bind(request_id)
                .bind(if attach_payment_hash { payment_hash.as_deref() } else { None })
                .bind(session_id.as_deref())
                .bind(reason)
                .bind(error_details)
                .bind(now)
                .execute(&mut *conn)
                .await
                .map_err(|e| Error::Internal(format!("Failed to create inference reconciliation item: {}", e)))?;
                Ok(item_id)
            }
        }
        .await;

        match transition_result {
            Ok(item_id) => {
                sqlx::query("COMMIT")
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| {
                        Error::Internal(format!(
                            "Failed to commit ambiguous request transition: {}",
                            e
                        ))
                    })?;
                Ok(item_id)
            }
            Err(error) => {
                if let Err(rollback_error) = sqlx::query("ROLLBACK").execute(&mut *conn).await {
                    tracing::error!(
                        "Failed to roll back ambiguous request transition: {}",
                        rollback_error
                    );
                }
                Err(error)
            }
        }
    }

    /// Startup recovery routine:
    ///
    /// Evaluates in-flight requests that were interrupted by a process termination.
    /// Returns a list of `(reservation_id, session_id)` pairs for requests that definitely did not begin dispatch (`started_at IS NULL`),
    /// allowing them to be safely released in `SessionStore`.
    ///
    /// Requests with `started_at IS NOT NULL` (which may have reached upstream) are marked `reconciliation_required`.
    pub async fn recover_in_flight_requests(&self) -> Result<Vec<(String, SessionId)>> {
        let now = chrono::Utc::now().timestamp();
        let mut definitely_undispatched_reservations = Vec::new();

        // 1. Fetch all requests not in a terminal state
        let query = r#"
            SELECT r.id, r.session_id, r.reservation_id, r.status, r.started_at, r.error_code
            FROM inference_requests r
            WHERE r.status IN ('reserved', 'processing', 'stream_interrupted')
               OR (r.status = 'reconciliation_required' AND NOT EXISTS (
                   SELECT 1 FROM reconciliation_items i
                   WHERE i.item_type = 'inference_request'
                     AND i.reference_id = r.id
                     AND i.status = 'unresolved'
               ))
        "#;

        type InFlightRequestRow = (
            String,
            Option<String>,
            Option<String>,
            String,
            Option<i64>,
            Option<String>,
        );

        let rows: Vec<InFlightRequestRow> = sqlx::query_as(query)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch in-flight requests: {}", e)))?;

        for (req_id, session_id_opt, res_id_opt, status_str, started_at_opt, error_code) in rows {
            if started_at_opt.is_none() && status_str == "reserved" {
                // Definitely undispatched
                sqlx::query(
                    "UPDATE inference_requests SET status = 'failed', failed_at = ?1, error_code = 'server_crash_prior_to_dispatch', error_category = 'startup_recovery' WHERE id = ?2"
                )
                .bind(now)
                .bind(&req_id)
                .execute(&self.pool)
                .await
                .map_err(|e| Error::Internal(format!("Failed to mark undispatched request failed: {}", e)))?;

                if let (Some(sid_str), Some(res_id)) = (session_id_opt, res_id_opt) {
                    if let Ok(uuid) = uuid::Uuid::parse_str(&sid_str) {
                        definitely_undispatched_reservations.push((res_id, SessionId(uuid)));
                    }
                }
            } else {
                // Ambiguous: dispatch may have occurred. Also repairs metadata on requests
                // already marked reconciliation_required if their item is missing.
                let reason = if status_str == "reconciliation_required" {
                    error_code
                        .as_deref()
                        .unwrap_or("unresolved_crash_during_execution")
                } else {
                    "unresolved_crash_during_execution"
                };
                self.mark_ambiguous_reconciliation_required(
                    &req_id,
                    reason,
                    "Node restarted while the request may have been dispatched upstream; execution outcome is unknown",
                )
                .await?;
            }
        }

        Ok(definitely_undispatched_reservations)
    }

    /// Count successfully completed inference requests for a given session.
    /// Excludes failed, reserved, processing, stream_interrupted, or reconciliation_required requests.
    pub async fn count_completed_by_session(&self, session_id: &SessionId) -> Result<u64> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM inference_requests WHERE session_id = ?1 AND status = 'completed'",
        )
        .bind(session_id.0.to_string())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("Failed to count completed requests for session: {}", e)))?;

        Ok(count.max(0) as u64)
    }

    /// Count total completed inference requests across all sessions and direct pay-per-request authorizations.
    /// Excludes failed, reserved, processing, stream_interrupted, or reconciliation_required requests.
    pub async fn count_completed_total(&self) -> Result<u64> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM inference_requests WHERE status = 'completed'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("Failed to count total completed requests: {}", e)))?;

        Ok(count.max(0) as u64)
    }

    /// Compute total sats earned from completed inference requests across the node.
    /// Excludes reservations, unresolved requests, or failed attempts.
    pub async fn total_completed_revenue_sats(&self) -> Result<u64> {
        let total: Option<i64> = sqlx::query_scalar(
            "SELECT SUM(charged_sats) FROM inference_requests WHERE status = 'completed'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            Error::Internal(format!("Failed to compute total completed revenue: {}", e))
        })?;

        Ok(total.unwrap_or(0).max(0) as u64)
    }
}
