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

pub async fn health_check(State(state): State<AppState>) -> impl IntoResponse {
    let backend_str = match state.config.lightning.backend {
        crate::config::schema::LightningBackendType::Mock => "mock",
        crate::config::schema::LightningBackendType::Lnd => "lnd_rest",
        crate::config::schema::LightningBackendType::Nwc => "nwc",
    };
    let network_str = match state.config.lightning.network {
        crate::config::schema::BitcoinNetwork::Mainnet => "mainnet",
        crate::config::schema::BitcoinNetwork::Testnet => "testnet",
        crate::config::schema::BitcoinNetwork::Regtest => "regtest",
        crate::config::schema::BitcoinNetwork::Signet => "signet",
    };

    Json(json!({
        "status": "ok",
        "service": "satsgrid-node",
        "lightning": {
            "backend": backend_str,
            "network": network_str,
        }
    }))
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

    // Register pending session and durable funding invoice atomically in SQLite
    let funding_expiry_secs = state.config.lightning.session_funding_expiry_secs;
    let expires_at = chrono::Utc::now().timestamp() + funding_expiry_secs as i64;
    state
        .budget_manager
        .create_pending_session_with_invoice(
            &session_id,
            &invoice.payment_hash,
            &invoice.bolt11,
            budget,
            expires_at,
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

        let ppr_expiry_secs = state.config.lightning.pay_per_request_expiry_secs;
        let invoice_expires_at = (now + ppr_expiry_secs) as i64;

        // Persist invoice in durable registry before emitting 402 challenge
        if let Some(db) = &state.db {
            db.invoice_store()
                .create_invoice(
                    &invoice.payment_hash,
                    &invoice.bolt11,
                    cost,
                    crate::node::storage::InvoicePurpose::PayPerRequest,
                    None,
                    invoice_expires_at,
                )
                .await?;
        }

        let mut caveats = vec![
            Caveat::Capability("inference".to_string()),
            Caveat::ExpiresAt(now + ppr_expiry_secs),
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
        // Step 2 & 2A: Lazy Session Activation with explicit payment-hash binding
        // If this is a pending-payment session whose invoice was verified as settled,
        // activate the session and record the initial funding ledger entry atomically,
        // strictly verifying that session.payment_hash == credentials.macaroon.payment_hash().
        state
            .budget_manager
            .activate_session_if_pending(session_id, &payment_hash)
            .await?;

        // Update invoice settlement status in durable registry if stored
        if let Some(db) = &state.db {
            let _ = db
                .invoice_store()
                .mark_settled(&payment_hash, now as i64)
                .await;
        }

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
    }

    // Stage 7.3 & 7.4: Concurrency Admission Gate
    // Placed strictly after authentication and pre-reservation idempotency inspection,
    // but strictly BEFORE any session budget reservation or payment authorization consumption.
    let concurrency_permit = state
        .concurrency_manager
        .acquire(session_opt.as_ref())
        .await?;

    if let Some(session_id) = &session_opt {
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
            if let Err(e) = req_store
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
                .await
            {
                if let Err(rel_err) = state
                    .budget_manager
                    .release_reservation(&res_id, "request_creation_failed")
                    .await
                {
                    tracing::error!(
                        reservation_id = %res_id,
                        error = %rel_err,
                        "Failed to release session reservation after request creation error"
                    );
                }
                return Err(e);
            }
        }
    } else {
        // Pay-per-request mode: enforce single-use authorization in persistent storage
        if let Some(db) = &state.db {
            let expires_at = credentials.macaroon.expires_at().unwrap_or(now + 3600);
            let fingerprint = validated.compute_canonical_fingerprint();

            let store = db.payment_store();
            let reservation = store
                .reserve_payment_hash_with_fingerprint(
                    &payment_hash,
                    &internal_request_id,
                    expires_at,
                    idempotency_key,
                    Some(&fingerprint),
                )
                .await?;

            match reservation {
                crate::node::storage::ReservationResult::Reserved => {}
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

            // Atomically consume payment authorization and mark inference request processing
            db.commit_authorization_and_mark_processing(&payment_hash, &internal_request_id)
                .await?;

            // Mark invoice settled in durable registry
            let _ = db
                .invoice_store()
                .mark_settled(&payment_hash, now as i64)
                .await;
        }
    }

    // For session requests: Durable Dispatch Boundary: Commit 'processing' state before upstream HTTP call
    if session_opt.is_some() {
        if let Some(db) = &state.db {
            db.request_store()
                .mark_processing(&internal_request_id)
                .await?;
        }
    }

    let upstream_dispatched = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    // RAII guard to safely handle client cancellations, timeouts, or unexpected task aborts
    // occurring while request is in the 'processing' state.
    struct DispatchCancellationGuard {
        request_id: String,
        session_reservation_id: Option<String>,
        db: Option<std::sync::Arc<crate::node::storage::Database>>,
        upstream_dispatched: std::sync::Arc<std::sync::atomic::AtomicBool>,
        defused: bool,
    }

    impl DispatchCancellationGuard {
        fn defuse(&mut self) {
            self.defused = true;
        }
    }

    impl Drop for DispatchCancellationGuard {
        fn drop(&mut self) {
            if !self.defused {
                if let Some(db) = self.db.take() {
                    let r_id = self.request_id.clone();
                    let s_res = self.session_reservation_id.clone();
                    let dispatched = self
                        .upstream_dispatched
                        .load(std::sync::atomic::Ordering::SeqCst);

                    tokio::spawn(async move {
                        let req_store = db.request_store();
                        if !dispatched {
                            // If we definitely know the request was not sent or failed before transmission,
                            // mark failed and release session reservation.
                            let _ = req_store
                                .mark_failed(&r_id, "connect_timeout_or_abort", "pre_dispatch")
                                .await;
                            if let Some(res_id) = s_res {
                                let _ = db
                                    .session_store()
                                    .release_reservation(&res_id, "connect_timeout_or_abort")
                                    .await;
                            }
                        } else {
                            // Upstream transmission was in flight; execution may have occurred.
                            // Conservative reconciliation required to protect session budget.
                            let _ = req_store
                                .mark_reconciliation_required(
                                    &r_id,
                                    "client_abort_or_timeout_after_dispatch",
                                )
                                .await;
                            if let Some(res_id) = s_res {
                                let _ = db
                                    .session_store()
                                    .mark_reconciliation_required(
                                        &res_id,
                                        "client_abort_or_timeout_after_dispatch",
                                    )
                                    .await;
                            }
                        }
                    });
                }
            }
        }
    }

    let mut dispatch_guard = DispatchCancellationGuard {
        request_id: internal_request_id.clone(),
        session_reservation_id: session_reservation_id.clone(),
        db: state.db.clone(),
        upstream_dispatched: upstream_dispatched.clone(),
        defused: false,
    };

    let total_timeout =
        std::time::Duration::from_secs(state.config.upstream.total_request_timeout_secs);
    let idle_timeout =
        std::time::Duration::from_secs(state.config.upstream.stream_idle_timeout_secs);

    if is_stream {
        upstream_dispatched.store(true, std::sync::atomic::Ordering::SeqCst);
        let stream_result = state.proxy.stream_chat_completion(validated.raw).await;

        let stream = match stream_result {
            Ok(s) => s,
            Err(e) => {
                // If it is an explicit connection failure, the request was definitely not executed
                if matches!(e, Error::UpstreamConnect(_)) {
                    dispatch_guard
                        .upstream_dispatched
                        .store(false, std::sync::atomic::Ordering::SeqCst);
                }
                // Dropping dispatch_guard here will safely handle transitions based on upstream_dispatched flag
                return Err(e);
            }
        };

        // Finalize session reservation upon establishing stream
        if let Some(res_id) = &session_reservation_id {
            let _ = state.budget_manager.finalize_reservation(res_id).await;
        }

        use std::future::Future;
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
            deadline: std::time::Instant,
            idle_sleep: Pin<Box<tokio::time::Sleep>>,
            idle_timeout: std::time::Duration,
            error_emitted: bool,
            _permit: crate::node::concurrency::ConcurrencyPermit,
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
                // 1. Check overall request deadline
                if std::time::Instant::now() >= self.deadline {
                    if !self.error_emitted {
                        self.error_emitted = true;
                        self.completed.store(true, Ordering::SeqCst);
                        if let Some(db) = &self.db {
                            let req_store = db.request_store();
                            let r_id = self.req_id.clone();
                            tokio::spawn(async move {
                                let _ = req_store
                                    .mark_stream_interrupted(&r_id, "stream_total_timeout", None)
                                    .await;
                            });
                        }
                        let sse_err = "event: error\ndata: {\"error\":{\"message\":\"Upstream total request deadline exceeded\",\"type\":\"upstream_timeout_error\",\"code\":\"stream_total_timeout\"}}\n\n";
                        return Poll::Ready(Some(Ok(bytes::Bytes::from(sse_err))));
                    }
                    return Poll::Ready(None);
                }

                // 2. Poll inner stream for new data
                match Pin::new(&mut self.inner).poll_next(cx) {
                    Poll::Ready(Some(Ok(bytes))) => {
                        // Reset idle sleep timer only upon actual data receipt!
                        let next_wake = tokio::time::Instant::now() + self.idle_timeout;
                        self.idle_sleep.as_mut().reset(next_wake);
                        Poll::Ready(Some(Ok(bytes)))
                    }
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
                    Poll::Pending => {
                        // 3. Poll idle sleep timer
                        match self.idle_sleep.as_mut().poll(cx) {
                            Poll::Ready(()) => {
                                // Idle timeout elapsed without data
                                if !self.error_emitted {
                                    self.error_emitted = true;
                                    self.completed.store(true, Ordering::SeqCst);
                                    if let Some(db) = &self.db {
                                        let req_store = db.request_store();
                                        let r_id = self.req_id.clone();
                                        tokio::spawn(async move {
                                            let _ = req_store
                                                .mark_stream_interrupted(
                                                    &r_id,
                                                    "stream_idle_timeout",
                                                    None,
                                                )
                                                .await;
                                        });
                                    }
                                    let sse_err = "event: error\ndata: {\"error\":{\"message\":\"Upstream stream idle timeout exceeded\",\"type\":\"upstream_timeout_error\",\"code\":\"stream_idle_timeout\"}}\n\n";
                                    return Poll::Ready(Some(Ok(bytes::Bytes::from(sse_err))));
                                }
                                Poll::Ready(None)
                            }
                            Poll::Pending => Poll::Pending,
                        }
                    }
                }
            }
        }

        let monitored_stream = MonitoredStream {
            inner: stream,
            req_id: req_id_clone,
            db: db_opt,
            prompt_tokens: prompt_tokens_i64,
            completed: std::sync::Arc::new(AtomicBool::new(false)),
            deadline: std::time::Instant::now() + total_timeout,
            idle_sleep: Box::pin(tokio::time::sleep(idle_timeout)),
            idle_timeout,
            error_emitted: false,
            _permit: concurrency_permit,
        };

        // MonitoredStream has assumed ownership; defuse dispatch guard
        dispatch_guard.defuse();

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
        // Retain concurrency permit in scope for non-streaming execution
        let _permit = concurrency_permit;

        // Non-streaming execution with total request deadline
        upstream_dispatched.store(true, std::sync::atomic::Ordering::SeqCst);
        let proxy_res = tokio::time::timeout(
            total_timeout,
            state
                .proxy
                .forward_chat_completion_with_headers(validated.raw, headers),
        )
        .await;

        let proxy_res = match proxy_res {
            Ok(res) => res,
            Err(_) => {
                return Err(Error::UpstreamTimeout(format!(
                    "Total request deadline exceeded after {:?}",
                    total_timeout
                )));
            }
        };

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

                // Atomic non-streaming finalization
                if let Some(db) = &state.db {
                    db.finalize_request_and_session(
                        &internal_request_id,
                        session_reservation_id.as_deref(),
                        prompt_tok,
                        comp_tok,
                    )
                    .await?;
                }

                // Successful durable finalization; defuse guard
                dispatch_guard.defuse();

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
                // If it is an explicit connection failure, the request was definitely not executed
                if matches!(e, Error::UpstreamConnect(_)) {
                    dispatch_guard
                        .upstream_dispatched
                        .store(false, std::sync::atomic::Ordering::SeqCst);
                }
                // Guard drop handles failure or reconciliation required
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

    let (pending_inv, settled_inv, unresolved_recon, worker_stats) = if let Some(db) = &state.db {
        let inv_store = db.invoice_store();
        let recon_store = db.reconciliation_store();
        let p = inv_store
            .count_by_status(crate::node::storage::InvoiceStatus::Pending)
            .await
            .unwrap_or(0);
        let s = inv_store
            .count_by_status(crate::node::storage::InvoiceStatus::Settled)
            .await
            .unwrap_or(0);
        let u = recon_store.count_unresolved().await.unwrap_or(0);
        let ws = if let Some(w) = AppState::reconciliation_worker() {
            let st = w.stats().await;
            Some(json!({
                "last_scan_timestamp": st.last_scan_timestamp,
                "scans_completed": st.scans_completed,
                "invoices_checked": st.invoices_checked,
                "invoices_settled": st.invoices_settled,
                "invoices_expired": st.invoices_expired,
                "items_reconciled": st.items_reconciled,
                "last_error": st.last_error,
            }))
        } else {
            None
        };
        (p, s, u, ws)
    } else {
        (0, 0, 0, None)
    };

    Ok(Json(json!({
        "total_requests": requests,
        "total_sats_earned": earned,
        "invoices": {
            "pending": pending_inv,
            "settled": settled_inv,
        },
        "reconciliation": {
            "unresolved_items": unresolved_recon,
            "worker": worker_stats,
        }
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
            Error::Upstream(_) | Error::UpstreamConnect(_) => {
                (StatusCode::BAD_GATEWAY, self.to_string())
            }
            Error::UpstreamTimeout(_) => {
                let err_resp = crate::node::api::validation::OpenAiErrorResponse::new(
                    self.to_string(),
                    "upstream_timeout_error",
                    None::<String>,
                    Some("upstream_timeout"),
                );
                return err_resp.to_response(StatusCode::GATEWAY_TIMEOUT);
            }
            Error::ServerOverloaded(_) => {
                let err_resp = crate::node::api::validation::OpenAiErrorResponse::new(
                    self.to_string(),
                    "server_overload_error",
                    None::<String>,
                    Some("node_capacity_exceeded"),
                );
                let mut resp = err_resp.to_response(StatusCode::SERVICE_UNAVAILABLE);
                resp.headers_mut()
                    .insert(axum::http::header::RETRY_AFTER, "1".parse().unwrap());
                return resp;
            }
            Error::ConcurrencyLimitExceeded(_) => {
                let err_resp = crate::node::api::validation::OpenAiErrorResponse::new(
                    self.to_string(),
                    "concurrency_limit_error",
                    None::<String>,
                    Some("session_concurrency_exceeded"),
                );
                let mut resp = err_resp.to_response(StatusCode::TOO_MANY_REQUESTS);
                resp.headers_mut()
                    .insert(axum::http::header::RETRY_AFTER, "1".parse().unwrap());
                return resp;
            }
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
