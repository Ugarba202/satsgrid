use super::AppState;
use crate::common::error::Error;
use crate::common::types::{Satoshis, SessionId};
use crate::node::gate::{Caveat, L402Challenge, L402Verifier};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

pub async fn health_check() -> impl IntoResponse {
    Json(json!({ "status": "ok", "service": "satsgrid-node" }))
}

pub async fn models(State(state): State<AppState>) -> impl IntoResponse {
    match state.proxy.list_models().await {
        Ok(upstream_models) => Json(upstream_models),
        Err(e) => {
            tracing::warn!(
                "Failed to query upstream models, falling back to default: {}",
                e
            );
            let data = vec![json!({
                "id": "llama3.2",
                "object": "model",
                "created": SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
                "owned_by": "satsgrid-node"
            })];
            Json(json!({ "object": "list", "data": data }))
        }
    }
}

#[derive(Deserialize)]
pub struct NewSessionRequest {
    pub budget_sats: u64,
}

pub async fn new_session(
    State(state): State<AppState>,
    crate::node::api::validation::BoundedJson(payload): crate::node::api::validation::BoundedJson<
        NewSessionRequest,
    >,
) -> Result<impl IntoResponse, Error> {
    if payload.budget_sats == 0 {
        return Err(Error::Internal(
            "Budget must be greater than 0 sats".to_string(),
        ));
    }

    let budget = Satoshis(payload.budget_sats);
    let invoice = state
        .lightning
        .create_invoice(budget, "SatsGrid Session Budget")
        .await
        .map_err(|e| {
            tracing::error!("Failed to create invoice via Lightning backend: {}", e);
            Error::Lightning(format!("Lightning node failed to create invoice: {}", e))
        })?;

    let uuid = uuid::Uuid::new_v4();
    let session_id = SessionId(uuid);

    // Register session in the budget manager (backed by SQLite when db is active)
    let default_expiry = chrono::Utc::now().timestamp() + 86400 * 30; // 30 days
    state
        .budget_manager
        .open_session_with_details(
            &session_id,
            Some(&invoice.payment_hash),
            budget,
            default_expiry,
        )
        .await?;

    // Mint macaroon with session caveats
    let caveats = vec![
        Caveat::Session(uuid.to_string()),
        Caveat::Budget(payload.budget_sats),
        Caveat::Capability("inference".to_string()),
    ];
    let macaroon = state
        .macaroon_service
        .mint(&invoice.payment_hash, caveats)
        .map_err(|e| {
            tracing::error!("Failed to mint session macaroon: {}", e);
            Error::Internal(e.to_string())
        })?;

    let challenge = L402Challenge::from_components(&macaroon, &invoice).map_err(|e| {
        tracing::error!("Failed to build L402 challenge: {}", e);
        Error::Internal(e.to_string())
    })?;

    // Respond with 402 Payment Required and the WWW-Authenticate challenge header
    let mut headers = HeaderMap::new();
    headers.insert(
        "WWW-Authenticate",
        challenge.to_header_value().parse().unwrap(),
    );

    Ok((
        StatusCode::PAYMENT_REQUIRED,
        headers,
        Json(json!({"status": "payment_required"})),
    ))
}

pub async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    crate::node::api::validation::BoundedJson(payload): crate::node::api::validation::BoundedJson<
        Value,
    >,
) -> Result<impl IntoResponse, Error> {
    // 1. Pre-auth Request Schema Validation
    // Validate model, messages, roles, content, parameters before inspecting credentials or reserving funds.
    let validation_rules = crate::node::api::validation::ChatValidationRules {
        max_completion_tokens_limit: state.config.server.max_completion_tokens_limit,
    };
    let validated =
        crate::node::api::validation::ValidatedChatRequest::validate(payload, &validation_rules)
            .map_err(Error::Validation)?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let prompt_tokens = validated.prompt_tokens;
    let max_completion_tokens = validated.max_tokens;
    let cost = state
        .pricing_calculator()
        .await
        .calculate_cost(prompt_tokens, max_completion_tokens);
    let req_model = Some(validated.model.as_str());

    let auth_header = headers.get("Authorization");
    if auth_header.is_none()
        || auth_header
            .unwrap()
            .to_str()
            .unwrap_or("")
            .trim()
            .is_empty()
    {
        // Issue standard L402 challenge for direct pay-per-request
        let invoice = state
            .lightning
            .create_invoice(cost, "SatsGrid Chat Completion")
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

        let mut caveats = vec![
            Caveat::Capability("inference".to_string()),
            Caveat::ExpiresAt(now + 3600),
        ];
        if let Some(model_name) = req_model {
            caveats.push(Caveat::Model(model_name.to_string()));
        }

        let macaroon = state
            .macaroon_service
            .mint(&invoice.payment_hash, caveats)
            .map_err(|e| Error::Internal(e.to_string()))?;

        let challenge = L402Challenge::from_components(&macaroon, &invoice)
            .map_err(|e| Error::Internal(e.to_string()))?;

        let mut challenge_headers = HeaderMap::new();
        challenge_headers.insert(
            "WWW-Authenticate",
            challenge.to_header_value().parse().unwrap(),
        );

        return Ok((
            StatusCode::PAYMENT_REQUIRED,
            challenge_headers,
            Json(json!({
                "error": "Payment required",
                "token": macaroon.to_base64().unwrap_or_default(),
                "invoice": invoice.bolt11
            })),
        )
            .into_response());
    }

    let auth_str = auth_header.unwrap().to_str().unwrap_or("");

    let credentials = L402Verifier::verify_header(
        &state.macaroon_service,
        &*state.lightning,
        auth_str,
        req_model,
        now,
    )
    .await
    .map_err(|_| Error::VerificationFailed("Invalid L402 credentials".to_string()))?;

    // Authorization: enforce endpoint capability
    if credentials.macaroon.capability().as_deref() != Some("inference") {
        return Err(Error::Forbidden(
            "Missing or invalid capability: requires 'inference'".to_string(),
        ));
    }

    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        "X-SatsGrid-Charged-Sats",
        cost.0.to_string().parse().unwrap(),
    );

    // Dual-mode authorization:
    // If the macaroon has a session caveat, debit from the session budget.
    // If no session caveat is present, this is a verified single pay-per-request credential.
    let (session_opt, _) =
        crate::node::gate::SessionBudgetManager::extract_session_caveats(&credentials.macaroon);

    let payment_hash = credentials.macaroon.payment_hash();

    let is_stream = validated.is_stream;

    let req_mode = if is_stream {
        crate::node::storage::RequestMode::Streaming
    } else {
        crate::node::storage::RequestMode::NonStreaming
    };

    let internal_request_id = uuid::Uuid::new_v4().to_string();
    let idempotency_key = headers
        .get("Idempotency-Key")
        .or_else(|| headers.get("X-Request-ID"))
        .and_then(|v| v.to_str().ok());

    let mut session_reservation_id: Option<String> = None;

    if let Some(session_id) = &session_opt {
        // Check session idempotency key if provided
        if let (Some(db), Some(idem_key)) = (&state.db, idempotency_key) {
            let req_store = db.request_store();
            if let Some(existing) = req_store
                .find_by_idempotency_key(session_id, idem_key)
                .await?
            {
                if existing.status == crate::node::storage::RequestStatus::Completed {
                    return Err(Error::VerificationFailed(
                        "Idempotency key has already been consumed for this session".to_string(),
                    ));
                } else if existing.status == crate::node::storage::RequestStatus::Processing
                    || existing.status == crate::node::storage::RequestStatus::Reserved
                {
                    return Err(Error::VerificationFailed(
                        "A request with this idempotency key is currently in flight".to_string(),
                    ));
                }
            }
        }

        // Two-phase session reservation: reserve budget first
        let res_id = uuid::Uuid::new_v4().to_string();
        let remaining = state
            .budget_manager
            .reserve_session(session_id, cost, &res_id)
            .await
            .map_err(|e| Error::BudgetExhausted(e.to_string()))?;

        session_reservation_id = Some(res_id.clone());

        response_headers.insert(
            "X-SatsGrid-Remaining-Budget-Sats",
            remaining.0.to_string().parse().unwrap(),
        );

        if let Some(db) = &state.db {
            let req_store = db.request_store();
            req_store
                .create_request(
                    &internal_request_id,
                    None,
                    Some(session_id),
                    Some(&res_id),
                    idempotency_key,
                    &validated.model,
                    req_mode,
                    cost,
                )
                .await?;
        }
    } else {
        // Pay-per-request mode: enforce single-use authorization in persistent storage
        if let Some(db) = &state.db {
            let expires_at = credentials.macaroon.expires_at().unwrap_or(now + 3600);

            let store = db.payment_store();
            let reservation = store
                .reserve_payment_hash(
                    &payment_hash,
                    &internal_request_id,
                    expires_at,
                    idempotency_key,
                )
                .await?;

            match reservation {
                crate::node::storage::ReservationResult::Reserved => {
                    store.mark_settled_and_consumed(&payment_hash).await?;
                }
                crate::node::storage::ReservationResult::AlreadyUsed {
                    state: auth_state, ..
                } => {
                    tracing::warn!(
                        payment_hash = %payment_hash.0,
                        state = %auth_state,
                        "Replay detected for pay-per-request payment hash"
                    );
                    return Err(Error::Unauthorized(
                        "Payment token has already been redeemed. Pay-per-request tokens are single-use.".to_string(),
                    ));
                }
            }

            let req_store = db.request_store();
            req_store
                .create_request(
                    &internal_request_id,
                    Some(&payment_hash),
                    None,
                    None,
                    idempotency_key,
                    &validated.model,
                    req_mode,
                    cost,
                )
                .await?;
        }
    }

    // Durable Dispatch Boundary: Commit 'processing' state before upstream HTTP call
    if let Some(db) = &state.db {
        db.request_store()
            .mark_processing(&internal_request_id)
            .await?;
    }

    if is_stream {
        let stream_result = state.proxy.stream_chat_completion(validated.raw).await;

        let stream = match stream_result {
            Ok(s) => s,
            Err(e) => {
                // Confirmed pre-generation failure before stream start
                if let Some(db) = &state.db {
                    let _ = db
                        .request_store()
                        .mark_failed(
                            &internal_request_id,
                            "upstream_connect_error",
                            "pre_dispatch",
                        )
                        .await;
                }
                if let Some(res_id) = &session_reservation_id {
                    let _ = state
                        .budget_manager
                        .release_reservation(res_id, "upstream_pre_generation_failure")
                        .await;
                }
                return Err(e);
            }
        };

        // Finalize session reservation upon establishing stream
        if let Some(res_id) = &session_reservation_id {
            let _ = state.budget_manager.finalize_reservation(res_id).await;
        }

        use std::pin::Pin;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::task::{Context, Poll};

        let db_opt = state.db.clone();
        let req_id_clone = internal_request_id.clone();
        let prompt_tokens_i64 = prompt_tokens as i64;

        struct MonitoredStream<S> {
            inner: S,
            req_id: String,
            db: Option<std::sync::Arc<crate::node::storage::Database>>,
            prompt_tokens: i64,
            completed: std::sync::Arc<AtomicBool>,
        }

        impl<S> Drop for MonitoredStream<S> {
            fn drop(&mut self) {
                // If stream is dropped before completion, mark stream_interrupted
                if !self.completed.load(Ordering::SeqCst) {
                    if let Some(db) = &self.db {
                        let req_store = db.request_store();
                        let r_id = self.req_id.clone();
                        tokio::spawn(async move {
                            let _ = req_store
                                .mark_stream_interrupted(&r_id, "client_disconnected_early", None)
                                .await;
                        });
                    }
                }
            }
        }

        impl<
                S: futures_util::Stream<Item = crate::common::error::Result<bytes::Bytes>> + Unpin,
            > futures_util::Stream for MonitoredStream<S>
        {
            type Item = crate::common::error::Result<bytes::Bytes>;

            fn poll_next(
                mut self: Pin<&mut Self>,
                cx: &mut Context<'_>,
            ) -> Poll<Option<Self::Item>> {
                match Pin::new(&mut self.inner).poll_next(cx) {
                    Poll::Ready(Some(Ok(bytes))) => Poll::Ready(Some(Ok(bytes))),
                    Poll::Ready(Some(Err(e))) => {
                        self.completed.store(true, Ordering::SeqCst);
                        if let Some(db) = &self.db {
                            let req_store = db.request_store();
                            let r_id = self.req_id.clone();
                            tokio::spawn(async move {
                                let _ = req_store
                                    .mark_stream_interrupted(&r_id, "stream_read_error", None)
                                    .await;
                            });
                        }
                        Poll::Ready(Some(Err(e)))
                    }
                    Poll::Ready(None) => {
                        // Stream finished normally!
                        if !self.completed.swap(true, Ordering::SeqCst) {
                            if let Some(db) = &self.db {
                                let req_store = db.request_store();
                                let r_id = self.req_id.clone();
                                let p_tok = self.prompt_tokens;
                                tokio::spawn(async move {
                                    let _ =
                                        req_store.mark_completed(&r_id, Some(p_tok), None).await;
                                });
                            }
                        }
                        Poll::Ready(None)
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
        }

        let monitored_stream = MonitoredStream {
            inner: stream,
            req_id: req_id_clone,
            db: db_opt,
            prompt_tokens: prompt_tokens_i64,
            completed: std::sync::Arc::new(AtomicBool::new(false)),
        };

        let body = axum::body::Body::from_stream(monitored_stream);
        let mut resp = body.into_response();
        resp.headers_mut().extend(response_headers);
        resp.headers_mut()
            .insert("content-type", "text/event-stream".parse().unwrap());

        state
            .stats
            .total_requests
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        state
            .stats
            .total_sats_earned
            .fetch_add(cost.0, std::sync::atomic::Ordering::SeqCst);

        Ok(resp)
    } else {
        // Proxy the request non-streaming
        let proxy_res = state
            .proxy
            .forward_chat_completion_with_headers(validated.raw, headers)
            .await;

        match proxy_res {
            Ok(proxy_resp) => {
                // Upstream success: extract tokens if available
                let (prompt_tok, comp_tok) = {
                    let usage = proxy_resp.get("usage");
                    let p = usage
                        .and_then(|u| u.get("prompt_tokens"))
                        .and_then(|v| v.as_i64());
                    let c = usage
                        .and_then(|u| u.get("completion_tokens"))
                        .and_then(|v| v.as_i64());
                    (p, c)
                };

                if let Some(db) = &state.db {
                    let _ = db
                        .request_store()
                        .mark_completed(&internal_request_id, prompt_tok, comp_tok)
                        .await;
                }

                if let Some(res_id) = &session_reservation_id {
                    let _ = state.budget_manager.finalize_reservation(res_id).await;
                }

                let mut resp = Json(proxy_resp).into_response();
                resp.headers_mut().extend(response_headers);

                state
                    .stats
                    .total_requests
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                state
                    .stats
                    .total_sats_earned
                    .fetch_add(cost.0, std::sync::atomic::Ordering::SeqCst);

                Ok(resp)
            }
            Err(e) => {
                // Confirmed upstream failure
                if let Some(db) = &state.db {
                    let _ = db
                        .request_store()
                        .mark_failed(&internal_request_id, "upstream_error", "proxy_forward")
                        .await;
                }

                // Release session reservation so user is not billed for failed execution
                if let Some(res_id) = &session_reservation_id {
                    let _ = state
                        .budget_manager
                        .release_reservation(res_id, "upstream_execution_failure")
                        .await;
                }

                Err(e)
            }
        }
    }
}

pub async fn node_stats(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, Error> {
    state.verify_admin(&headers)?;

    let requests = state
        .stats
        .total_requests
        .load(std::sync::atomic::Ordering::SeqCst);
    let earned = state
        .stats
        .total_sats_earned
        .load(std::sync::atomic::Ordering::SeqCst);

    Ok(Json(json!({
        "total_requests": requests,
        "total_sats_earned": earned,
    })))
}

pub async fn node_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, Error> {
    state.verify_admin(&headers)?;

    let pricing = state.live_pricing.read().await.clone();
    Ok(Json(json!({
        "pricing": {
            "default_price_sats": pricing.default_price_sats,
            "sats_per_prompt_token": pricing.sats_per_prompt_token,
            "sats_per_completion_token": pricing.sats_per_completion_token,
        },
        "upstream": {
            "url": state.config.upstream.url
        }
    })))
}

pub async fn update_node_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<crate::config::schema::PricingConfig>,
) -> Result<impl IntoResponse, Error> {
    state.verify_admin(&headers)?;

    let mut pricing = state.live_pricing.write().await;
    *pricing = payload.clone();

    tracing::info!(
        default_price_sats = payload.default_price_sats.0,
        sats_per_prompt_token = payload.sats_per_prompt_token,
        sats_per_completion_token = payload.sats_per_completion_token,
        "Node pricing updated via admin endpoint"
    );

    Ok(Json(json!({
        "status": "success",
        "pricing": payload
    })))
}

// Error Mapping for Axum
impl IntoResponse for Error {
    fn into_response(self) -> axum::response::Response {
        let (status, err_msg) = match &self {
            Error::PaymentRequired { invoice, token } => {
                let challenge = format!("L402 token=\"{}\", invoice=\"{}\"", token, invoice);
                let mut headers = HeaderMap::new();
                headers.insert("WWW-Authenticate", challenge.parse().unwrap());
                return (
                    StatusCode::PAYMENT_REQUIRED,
                    headers,
                    Json(json!({"error": self.to_string()})),
                )
                    .into_response();
            }
            Error::SessionRequired => (StatusCode::PAYMENT_REQUIRED, self.to_string()),
            Error::Unauthorized(_) | Error::VerificationFailed(_) => {
                (StatusCode::UNAUTHORIZED, self.to_string())
            }
            Error::Forbidden(_) => (StatusCode::FORBIDDEN, self.to_string()),
            Error::BudgetExhausted(_) => (StatusCode::PAYMENT_REQUIRED, self.to_string()),
            Error::Upstream(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            Error::Lightning(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            Error::Validation(val_err) => {
                return val_err.to_response(StatusCode::BAD_REQUEST);
            }
            Error::Config(_) | Error::Internal(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, self.to_string())
            }
        };
        (status, Json(json!({"error": err_msg}))).into_response()
    }
}

#[derive(Deserialize)]
pub struct MockPayRequest {
    pub invoice: String,
}

pub async fn mock_pay(
    State(state): State<AppState>,
    Json(payload): Json<MockPayRequest>,
) -> Result<impl IntoResponse, Error> {
    if state.config.lightning.backend != crate::config::schema::LightningBackendType::Mock {
        return Err(Error::Forbidden(
            "Mock payment endpoint is only available when lightning backend is 'mock'".to_string(),
        ));
    }

    let preimage = state
        .lightning
        .pay_invoice(&payload.invoice)
        .await
        .map_err(|e| Error::Lightning(e.to_string()))?;

    Ok(Json(json!({
        "preimage": preimage
    })))
}
