use super::AppState;
use crate::common::error::{Error, Result};
use crate::common::types::SessionId;
use crate::node::gate::{L402Verifier, SessionBudgetManager};
use crate::node::storage::SessionStatus;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct SessionReadResponse {
    pub session_id: String,
    pub status: &'static str,
    pub total_budget_sats: u64,
    pub remaining_sats: u64,
    pub reserved_sats: u64,
    pub spent_sats: u64,
    pub requests_count: u64,
    pub created_at: i64,
    pub expires_at: i64,
    pub closed_at: Option<i64>,
}

/// Read a session's persisted accounting after verifying the caller's L402 credentials.
pub async fn get_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> Result<impl IntoResponse> {
    let session_uuid = Uuid::parse_str(&session_id)
        .map(SessionId)
        .map_err(|_| Error::NotFound("Session not found".to_string()))?;
    let auth_header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|header| header.to_str().ok())
        .ok_or_else(|| Error::Unauthorized("L402 authorization required".to_string()))?;

    if !(auth_header.starts_with("L402 ") || auth_header.starts_with("LSAT ")) {
        return Err(Error::Unauthorized(
            "L402 authorization required for session access".to_string(),
        ));
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::Internal("System clock is before Unix epoch".to_string()))?
        .as_secs();
    let credentials = L402Verifier::verify_header(
        &state.macaroon_service,
        &*state.lightning,
        auth_header,
        None,
        now,
    )
    .await
    .map_err(|_| Error::VerificationFailed("Invalid L402 credentials".to_string()))?;

    let (authorized_session, _) =
        SessionBudgetManager::extract_session_caveats(&credentials.macaroon);
    if authorized_session.as_ref() != Some(&session_uuid) {
        return Err(Error::Forbidden(
            "L402 credentials are not authorized for this session".to_string(),
        ));
    }

    let database = state
        .db
        .as_ref()
        .ok_or_else(|| Error::Internal("Session storage is unavailable".to_string()))?;
    let session = database
        .session_store()
        .get_session(&session_uuid)
        .await?
        .ok_or_else(|| Error::NotFound("Session not found".to_string()))?;

    let requests_count = database
        .request_store()
        .count_completed_by_session(&session_uuid)
        .await?;

    let effective_status =
        if session.status == SessionStatus::Active && session.expires_at <= now as i64 {
            "expired"
        } else {
            session.status.as_str()
        };

    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );

    Ok((
        StatusCode::OK,
        response_headers,
        axum::Json(SessionReadResponse {
            session_id: session.id.0.to_string(),
            status: effective_status,
            total_budget_sats: session.total_budget_sats.0,
            remaining_sats: session.remaining_sats.0,
            reserved_sats: session.reserved_sats.0,
            spent_sats: session.spent_sats.0,
            requests_count,
            created_at: session.created_at,
            expires_at: session.expires_at,
            closed_at: session.closed_at,
        }),
    ))
}
