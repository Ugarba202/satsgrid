use super::AppState;
use crate::common::error::{Error, Result};
use crate::common::types::SessionId;
use crate::node::api::validation::OpenAiErrorResponse;
use crate::node::gate::{L402Verifier, SessionBudgetManager};
use crate::node::storage::SessionCloseResult;
use axum::{
    extract::{Json, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Deserialize)]
pub struct CloseSessionRequest {
    pub session_id: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CloseSessionResponse {
    pub session_id: String,
    pub status: String,
    pub total_budget_sats: u64,
    pub remaining_sats: u64,
    pub reserved_sats: u64,
    pub spent_sats: u64,
    pub closed_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Handler for POST /v1/session/close
///
/// Dual-mode authorization:
/// 1. L402 authorization: Caller proves possession of the session macaroon and settled preimage.
///    The session caveat inside the macaroon dictates the target session. If the body contains
///    a session_id, it must match the caveat.
/// 2. Admin authorization: Caller supplies `Authorization: Bearer <ADMIN_TOKEN>`.
///    Admin must supply `session_id` in the request body.
pub async fn close_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<CloseSessionRequest>,
) -> Result<impl IntoResponse> {
    let auth_header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| {
            Error::Unauthorized("Authorization header missing for session closure".to_string())
        })?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Determine authorization type: Admin vs L402
    let target_session_id = if auth_header.starts_with("Bearer ") {
        // Admin token authorization
        state.verify_admin(&headers)?;

        let session_str = payload.session_id.as_deref().ok_or_else(|| {
            Error::Validation(OpenAiErrorResponse::invalid_request(
                "session_id required in request body when closing session as admin",
                Some("session_id"),
                Some("missing_required_field"),
            ))
        })?;

        let parsed_uuid = uuid::Uuid::parse_str(session_str).map_err(|_| {
            Error::Validation(OpenAiErrorResponse::invalid_request(
                "Invalid UUID format for session_id",
                Some("session_id"),
                Some("invalid_uuid"),
            ))
        })?;

        SessionId(parsed_uuid)
    } else if auth_header.starts_with("L402 ") || auth_header.starts_with("LSAT ") {
        // L402 credentials authorization
        let credentials = L402Verifier::verify_header(
            &state.macaroon_service,
            &*state.lightning,
            auth_header,
            None,
            now,
        )
        .await
        .map_err(|_| Error::VerificationFailed("Invalid L402 credentials".to_string()))?;

        let (extracted_session, _) =
            SessionBudgetManager::extract_session_caveats(&credentials.macaroon);

        let macaroon_session_id = extracted_session.ok_or_else(|| {
            Error::Forbidden("Macaroon does not contain a session identifier caveat".to_string())
        })?;

        // If caller also supplied session_id in payload, enforce consistency
        if let Some(ref body_session_str) = payload.session_id {
            if let Ok(body_uuid) = uuid::Uuid::parse_str(body_session_str) {
                if SessionId(body_uuid) != macaroon_session_id {
                    return Err(Error::Forbidden(
                        "Supplied session_id does not match the session in the L402 credentials"
                            .to_string(),
                    ));
                }
            } else {
                return Err(Error::Validation(OpenAiErrorResponse::invalid_request(
                    "Invalid UUID format for session_id",
                    Some("session_id"),
                    Some("invalid_uuid"),
                )));
            }
        }

        macaroon_session_id
    } else {
        return Err(Error::Unauthorized(
            "Unsupported authorization scheme for session closure".to_string(),
        ));
    };

    let reason = payload.reason.as_deref();

    // Execute atomic closure in storage via BudgetManager
    let close_res = state
        .budget_manager
        .close_session(&target_session_id, reason)
        .await?;

    match close_res {
        SessionCloseResult::Closed {
            total_budget_sats,
            remaining_sats,
            reserved_sats,
            spent_sats,
            closed_at,
        } => {
            let resp = CloseSessionResponse {
                session_id: target_session_id.0.to_string(),
                status: "closed".to_string(),
                total_budget_sats: total_budget_sats.0,
                remaining_sats: remaining_sats.0,
                reserved_sats: reserved_sats.0,
                spent_sats: spent_sats.0,
                closed_at: Some(closed_at),
                reason: reason.map(|r| r.to_string()),
            };
            Ok((StatusCode::OK, Json(resp)).into_response())
        }
        SessionCloseResult::AlreadyClosed { closed_at } => {
            // Fetch session details for accurate accounting response
            let (total, rem, res, spent, reason_str) = if let Some(ref db) = state.db {
                if let Ok(Some(s)) = db.session_store().get_session(&target_session_id).await {
                    (
                        s.total_budget_sats.0,
                        s.remaining_sats.0,
                        s.reserved_sats.0,
                        s.spent_sats.0,
                        s.closed_reason,
                    )
                } else {
                    (0, 0, 0, 0, None)
                }
            } else {
                (0, 0, 0, 0, None)
            };

            let resp = CloseSessionResponse {
                session_id: target_session_id.0.to_string(),
                status: "already_closed".to_string(),
                total_budget_sats: total,
                remaining_sats: rem,
                reserved_sats: res,
                spent_sats: spent,
                closed_at,
                reason: reason_str,
            };
            Ok((StatusCode::OK, Json(resp)).into_response())
        }
        SessionCloseResult::NotFound => Err(Error::VerificationFailed(format!(
            "Session '{}' not found",
            target_session_id.0
        ))),
        SessionCloseResult::ActiveRequestsPending { reserved_sats } => {
            Err(Error::Internal(format!(
                "Cannot close session '{}': {} sats currently reserved by active requests",
                target_session_id.0, reserved_sats.0
            )))
        }
    }
}
