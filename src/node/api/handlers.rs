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

async fn persist_stream_completion(
    db: std::sync::Arc<crate::node::storage::Database>,
    stats: std::sync::Arc<super::NodeStats>,
    request_id: String,
    reservation_id: Option<String>,
    prompt_tokens: i64,
    charge_sats: u64,
    receipt: std::result::Result<Option<crate::node::receipt::SignedReceipt>, Error>,
) -> Option<String> {
    let receipt = match receipt {
        Ok(receipt) => receipt,
        Err(error) => {
            tracing::error!(request_id = %request_id, error = %error, "Failed to sign completed streaming request receipt");
            if let Err(recovery_error) = db
                .request_store()
                .mark_stream_finalization_recovery_required(
                    &request_id,
                    "stream_receipt_signing_failed",
                    "Streaming reached [DONE], but receipt signing failed; the reservation remains protected for recovery",
                )
                .await
            {
                tracing::error!(request_id = %request_id, error = %recovery_error, "Failed to persist streaming receipt-signing recovery state");
            }
            return None;
        }
    };

    let finalization = if let Some(receipt) = receipt.as_ref() {
        db.complete_streaming_request_with_receipt(
            &request_id,
            reservation_id.as_deref(),
            Some(prompt_tokens),
            None,
            receipt,
        )
        .await
    } else {
        db.finalize_request_and_session(
            &request_id,
            reservation_id.as_deref(),
            Some(prompt_tokens),
            None,
        )
        .await
    };

    if let Err(error) = finalization {
        // A commit error can be ambiguous: inspect durable state before deciding whether
        // to record recovery metadata. The transaction makes request, reservation and
        // receipt visible together when it did commit.
        let completed_durably = match db.request_store().get_request(&request_id).await {
            Ok(Some(request))
                if request.status == crate::node::storage::RequestStatus::Completed =>
            {
                if receipt.is_some() {
                    matches!(
                        db.receipt_store()
                            .get_receipt_by_request_id(&request_id)
                            .await,
                        Ok(Some(_))
                    )
                } else {
                    true
                }
            }
            Ok(_) => false,
            Err(check_error) => {
                tracing::error!(request_id = %request_id, error = %check_error, "Failed to inspect streaming finalization state after an error");
                false
            }
        };

        if !completed_durably {
            tracing::error!(request_id = %request_id, error = %error, "Failed to durably finalize completed streaming request");
            if let Err(recovery_error) = db
                .request_store()
                .mark_stream_finalization_recovery_required(
                    &request_id,
                    "stream_finalization_failed",
                    "Streaming reached [DONE], but durable request, reservation, or receipt finalization failed; the reservation remains protected for recovery",
                )
                .await
            {
                tracing::error!(request_id = %request_id, error = %recovery_error, "Failed to persist streaming finalization recovery state");
            }
            return None;
        }
    }

    stats
        .total_requests
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    stats
        .total_sats_earned
        .fetch_add(charge_sats, std::sync::atomic::Ordering::SeqCst);

    receipt.map(|r| r.payload.receipt_id)
}

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
        "version": env!("CARGO_PKG_VERSION"),
        "lightning": {
            "backend": backend_str,
            "network": network_str,
        }
    }))
}

pub async fn list_providers(State(state): State<AppState>) -> impl IntoResponse {
    if let Some(db) = &state.db {
        match db.provider_store().list_providers(None).await {
            Ok(providers) => {
                let data: Vec<Value> = providers
                    .into_iter()
                    .map(|p| {
                        json!({
                            "pubkey": p.pubkey,
                            "name": p.name,
                            "endpoint": p.endpoint,
                            "models": p.models,
                            "pricing_sats": p.pricing_sats,
                            "network": p.network,
                            "version": p.version,
                            "receipt_key_id": p.receipt_key_id,
                            "status": p.status,
                            "last_seen_at": p.last_seen_at,
                            "last_checked_at": p.last_checked_at,
                            "last_error": p.last_error,
                        })
                    })
                    .collect();
                (StatusCode::OK, Json(json!({ "providers": data }))).into_response()
            }
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Failed to list providers: {}", e) })),
            )
                .into_response(),
        }
    } else {
        (StatusCode::OK, Json(json!({ "providers": [] }))).into_response()
    }
}

pub async fn get_provider(
    State(state): State<AppState>,
    axum::extract::Path(pubkey): axum::extract::Path<String>,
) -> impl IntoResponse {
    if let Some(db) = &state.db {
        match db.provider_store().get_provider(&pubkey).await {
            Ok(Some(p)) => (
                StatusCode::OK,
                Json(json!({
                    "pubkey": p.pubkey,
                    "name": p.name,
                    "endpoint": p.endpoint,
                    "models": p.models,
                    "pricing_sats": p.pricing_sats,
                    "network": p.network,
                    "version": p.version,
                    "receipt_key_id": p.receipt_key_id,
                    "status": p.status,
                    "last_seen_at": p.last_seen_at,
                    "last_checked_at": p.last_checked_at,
                    "last_error": p.last_error,
                })),
            )
                .into_response(),
            Ok(None) => (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("Provider '{}' not found", pubkey) })),
            )
                .into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Failed to get provider: {}", e) })),
            )
                .into_response(),
        }
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Provider store unavailable" })),
        )
            .into_response()
    }
}

pub async fn models(State(state): State<AppState>) -> impl IntoResponse {
    if let Some(db) = &state.db {
        let model_store = db.model_store();
        match model_store.list_models(Some(true), Some(true)).await {
            Ok(models) if !models.is_empty() => {
                let data: Vec<Value> = models
                    .into_iter()
                    .map(|m| {
                        json!({
                            "id": m.id,
                            "object": "model",
                            "created": m.created_at,
                            "owned_by": m.owned_by,
                            "description": m.description,
                            "context_length": m.context_length,
                            "pricing": {
                                "flat_rate_sats": m.flat_rate_sats,
                                "sats_per_prompt_token": m.sats_per_prompt_token,
                                "sats_per_completion_token": m.sats_per_completion_token,
                            }
                        })
                    })
                    .collect();
                return Json(json!({ "object": "list", "data": data }));
            }
            Ok(_) => {
                // Store is empty: attempt lazy synchronization from upstream proxy
                if let Ok(val) = state.proxy.list_models().await {
                    let mut discovered_models = Vec::new();
                    if let Some(arr) = val.get("data").and_then(|d| d.as_array()) {
                        for item in arr {
                            if let Some(id) = item.get("id").and_then(|s| s.as_str()) {
                                discovered_models.push(
                                    crate::node::storage::model::DiscoveredModel {
                                        id: id.to_string(),
                                        name: item
                                            .get("name")
                                            .and_then(|s| s.as_str())
                                            .unwrap_or(id)
                                            .to_string(),
                                        owned_by: item
                                            .get("owned_by")
                                            .and_then(|s| s.as_str())
                                            .map(|s| s.to_string()),
                                        description: item
                                            .get("description")
                                            .and_then(|s| s.as_str())
                                            .map(|s| s.to_string()),
                                        context_length: item
                                            .get("context_length")
                                            .and_then(|v| v.as_i64()),
                                    },
                                );
                            }
                        }
                    }
                    if !discovered_models.is_empty() {
                        let _ = model_store
                            .upsert_discovered(&discovered_models, false)
                            .await;
                        if let Ok(fresh) = model_store.list_models(Some(true), Some(true)).await {
                            if !fresh.is_empty() {
                                let data: Vec<Value> = fresh
                                    .into_iter()
                                    .map(|m| {
                                        json!({
                                            "id": m.id,
                                            "object": "model",
                                            "created": m.created_at,
                                            "owned_by": m.owned_by,
                                            "description": m.description,
                                            "context_length": m.context_length,
                                            "pricing": {
                                                "flat_rate_sats": m.flat_rate_sats,
                                                "sats_per_prompt_token": m.sats_per_prompt_token,
                                                "sats_per_completion_token": m.sats_per_completion_token,
                                            }
                                        })
                                    })
                                    .collect();
                                return Json(json!({ "object": "list", "data": data }));
                            }
                        }
                    }
                }
                return Json(json!({ "object": "list", "data": [] }));
            }
            Err(e) => {
                tracing::error!("Failed to query ModelStore for /v1/models: {}", e);
                return Json(json!({ "object": "list", "data": [] }));
            }
        }
    }

    // Fallback if no database handle is configured (e.g. lightweight unit test harnesses)
    match state.proxy.list_models().await {
        Ok(upstream_models) => Json(upstream_models),
        Err(_) => Json(json!({ "object": "list", "data": [] })),
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

    // Check durable ModelStore for model record and pricing overrides
    let model_record = if let Some(db) = &state.db {
        db.model_store()
            .get_model(&validated.model)
            .await
            .unwrap_or(None)
    } else {
        None
    };

    let cost = state.pricing_calculator().await.calculate_cost_for_model(
        model_record.as_ref(),
        prompt_tokens,
        max_completion_tokens,
    );
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
                            if let Err(error) = req_store
                                .mark_failed(&r_id, "connect_timeout_or_abort", "pre_dispatch")
                                .await
                            {
                                tracing::error!(request_id = %r_id, %error, "Failed to mark pre-dispatch request failed");
                            }
                            if let Some(res_id) = s_res {
                                if let Err(error) = db
                                    .session_store()
                                    .release_reservation(&res_id, "connect_timeout_or_abort")
                                    .await
                                {
                                    tracing::error!(reservation_id = %res_id, %error, "Failed to release definitely undispatched reservation");
                                }
                            }
                        } else {
                            // Upstream transmission was in flight; execution may have occurred.
                            // Conservative reconciliation required to protect session budget.
                            if let Err(error) = req_store
                                .mark_ambiguous_reconciliation_required(
                                    &r_id,
                                    "client_abort_or_timeout_after_dispatch",
                                    "Client disconnected or request timed out after upstream dispatch; execution outcome is unknown",
                                )
                                .await
                            {
                                tracing::error!(request_id = %r_id, %error, "Failed to atomically mark ambiguous request, reconciliation item, and reservation");
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

        use std::future::Future;
        use std::pin::Pin;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::task::{Context, Poll};

        let db_opt = state.db.clone();
        let req_id_clone = internal_request_id.clone();
        let session_id_for_stream = session_opt.clone();
        let session_reservation_for_stream = session_reservation_id.clone();
        let model_for_stream = validated.model.clone();
        let payment_hash_for_stream = payment_hash.clone();
        let charge_sats_for_stream = cost.0;
        let prompt_tokens_i64 = prompt_tokens as i64;
        let stats_for_stream = state.stats.clone();

        fn strip_done_marker(tail: &mut Vec<u8>, bytes: &[u8]) -> (Option<Vec<u8>>, bool) {
            let mut combined = std::mem::take(tail);
            let tail_len = combined.len();
            combined.extend_from_slice(bytes);

            if let Some(idx) = combined
                .windows(b"[DONE]".len())
                .position(|w| w == b"[DONE]")
            {
                let line_start = combined[..idx].iter().rposition(|&b| b == b'\n');
                let prefix = match line_start {
                    Some(pos) => {
                        if pos + 1 <= tail_len {
                            Vec::new()
                        } else {
                            combined[tail_len..=pos].to_vec()
                        }
                    }
                    None => Vec::new(),
                };
                (Some(prefix), true)
            } else {
                let new_tail_start = combined.len().saturating_sub(b"[DONE]".len() - 1);
                *tail = combined[new_tail_start..].to_vec();
                (Some(bytes.to_vec()), false)
            }
        }

        struct MonitoredStream<S> {
            inner: S,
            req_id: String,
            session_id: Option<SessionId>,
            session_reservation_id: Option<String>,
            model: String,
            payment_hash: crate::common::types::PaymentHash,
            charge_sats: u64,
            db: Option<std::sync::Arc<crate::node::storage::Database>>,
            stats: std::sync::Arc<super::NodeStats>,
            prompt_tokens: i64,
            completed: std::sync::Arc<AtomicBool>,
            saw_done_marker: bool,
            terminal_marker_tail: Vec<u8>,
            deadline: std::time::Instant,
            idle_sleep: Pin<Box<tokio::time::Sleep>>,
            idle_timeout: std::time::Duration,
            error_emitted: bool,
            _permit: crate::node::concurrency::ConcurrencyPermit,
            finalizer: Option<Pin<Box<dyn std::future::Future<Output = Option<String>> + Send>>>,
            pending_emission: Option<bytes::Bytes>,
            done_emitted: bool,
        }

        impl<S> Drop for MonitoredStream<S> {
            fn drop(&mut self) {
                // If stream is dropped before completion, mark stream_interrupted
                if !self.completed.load(Ordering::SeqCst) {
                    if let Some(db) = &self.db {
                        let req_store = db.request_store();
                        let recon_store = db.reconciliation_store();
                        let r_id = self.req_id.clone();
                        let session_id = self.session_id.clone();
                        tokio::spawn(async move {
                            if let Err(error) = req_store
                                .mark_stream_interrupted(&r_id, "client_disconnected_early", None)
                                .await
                            {
                                tracing::error!(request_id = %r_id, %error, "Failed to record interrupted stream request");
                            }
                            if let Err(error) = recon_store.create_or_update_item(
                                crate::node::storage::ReconciliationItemType::InferenceRequest,
                                &r_id,
                                None,
                                session_id.as_ref(),
                                "client_disconnected_early",
                                Some("Client disconnected after upstream stream dispatch; execution outcome is unknown"),
                                None,
                            ).await {
                                tracing::error!(request_id = %r_id, %error, "Failed to persist interrupted stream reconciliation item");
                            }
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
                // 1. Yield any buffered pending emission (e.g. terminal [DONE] after receipt)
                if let Some(pending) = self.pending_emission.take() {
                    return Poll::Ready(Some(Ok(pending)));
                }

                // 2. If [DONE] was already emitted, the stream is finished
                if self.done_emitted {
                    return Poll::Ready(None);
                }

                // 3. If finalizer future is running, poll it
                if let Some(mut fut) = self.finalizer.take() {
                    match fut.as_mut().poll(cx) {
                        Poll::Pending => {
                            self.finalizer = Some(fut);
                            return Poll::Pending;
                        }
                        Poll::Ready(receipt_id_opt) => {
                            self.completed.store(true, Ordering::SeqCst);
                            self.done_emitted = true;
                            if let Some(receipt_id) = receipt_id_opt {
                                let sse_receipt = format!(
                                    "event: receipt\ndata: {{\"choices\":[],\"receipt_id\":\"{}\"}}\n\n",
                                    receipt_id
                                );
                                self.pending_emission =
                                    Some(bytes::Bytes::from("data: [DONE]\n\n"));
                                return Poll::Ready(Some(Ok(bytes::Bytes::from(sse_receipt))));
                            } else {
                                return Poll::Ready(Some(Ok(bytes::Bytes::from(
                                    "data: [DONE]\n\n",
                                ))));
                            }
                        }
                    }
                }

                // 4. Check overall request deadline
                if std::time::Instant::now() >= self.deadline {
                    if !self.error_emitted {
                        self.error_emitted = true;
                        self.completed.store(true, Ordering::SeqCst);
                        if let Some(db) = &self.db {
                            let req_store = db.request_store();
                            let recon_store = db.reconciliation_store();
                            let r_id = self.req_id.clone();
                            let session_id = self.session_id.clone();
                            tokio::spawn(async move {
                                if let Err(error) = req_store
                                    .mark_stream_interrupted(&r_id, "stream_total_timeout", None)
                                    .await
                                {
                                    tracing::error!(request_id = %r_id, %error, "Failed to record interrupted stream request");
                                }
                                if let Err(error) = recon_store.create_or_update_item(
                                    crate::node::storage::ReconciliationItemType::InferenceRequest,
                                    &r_id,
                                    None,
                                    session_id.as_ref(),
                                    "stream_total_timeout",
                                    Some("Upstream stream exceeded the request deadline after dispatch; execution outcome is unknown"),
                                    None,
                                ).await {
                                    tracing::error!(request_id = %r_id, %error, "Failed to persist interrupted stream reconciliation item");
                                }
                            });
                        }
                        let sse_err = "event: error\ndata: {\"error\":{\"message\":\"Upstream total request deadline exceeded\",\"type\":\"upstream_timeout_error\",\"code\":\"stream_total_timeout\"}}\n\n";
                        return Poll::Ready(Some(Ok(bytes::Bytes::from(sse_err))));
                    }
                    return Poll::Ready(None);
                }

                // 5. Poll inner stream for new data
                match Pin::new(&mut self.inner).poll_next(cx) {
                    Poll::Ready(Some(Ok(bytes))) => {
                        let mut tail = std::mem::take(&mut self.terminal_marker_tail);
                        let (prefix_opt, has_done) = strip_done_marker(&mut tail, &bytes);
                        self.terminal_marker_tail = tail;
                        if has_done {
                            self.saw_done_marker = true;
                        }

                        // Reset idle sleep timer only upon actual data receipt!
                        let next_wake = tokio::time::Instant::now() + self.idle_timeout;
                        self.idle_sleep.as_mut().reset(next_wake);

                        if let Some(prefix) = prefix_opt {
                            if !prefix.is_empty() {
                                return Poll::Ready(Some(Ok(bytes::Bytes::from(prefix))));
                            }
                        }

                        if has_done {
                            cx.waker().wake_by_ref();
                        }
                        Poll::Pending
                    }
                    Poll::Ready(Some(Err(e))) => {
                        self.completed.store(true, Ordering::SeqCst);
                        if let Some(db) = &self.db {
                            let req_store = db.request_store();
                            let recon_store = db.reconciliation_store();
                            let r_id = self.req_id.clone();
                            let session_id = self.session_id.clone();
                            tokio::spawn(async move {
                                if let Err(error) = req_store
                                    .mark_stream_interrupted(&r_id, "stream_read_error", None)
                                    .await
                                {
                                    tracing::error!(request_id = %r_id, %error, "Failed to record interrupted stream request");
                                }
                                if let Err(error) = recon_store.create_or_update_item(
                                    crate::node::storage::ReconciliationItemType::InferenceRequest,
                                    &r_id,
                                    None,
                                    session_id.as_ref(),
                                    "stream_read_error",
                                    Some("Upstream stream failed after dispatch; execution outcome is unknown"),
                                    None,
                                ).await {
                                    tracing::error!(request_id = %r_id, %error, "Failed to persist interrupted stream reconciliation item");
                                }
                            });
                        }
                        Poll::Ready(Some(Err(e)))
                    }
                    Poll::Ready(None) => {
                        // Stream finished normally!
                        // A clean transport EOF without the provider's SSE terminal marker can
                        // still represent a truncated response; keep it out of success finalization.
                        if !self.saw_done_marker {
                            return Poll::Ready(None);
                        }

                        if let Some(db) = self.db.clone() {
                            let r_id = self.req_id.clone();
                            let p_tok = self.prompt_tokens;
                            let session_id = self.session_id.clone();
                            let reservation_id = self.session_reservation_id.clone();
                            let model = self.model.clone();
                            let payment_hash = self.payment_hash.clone();
                            let charge_sats = self.charge_sats;
                            let stats = self.stats.clone();
                            let fut = Box::pin(async move {
                                let receipt = if let Some(signer) = AppState::receipt_signer() {
                                    let receipt_id =
                                        format!("rcpt_{}", uuid::Uuid::new_v4().simple());
                                    let payload = crate::node::receipt::ReceiptPayload {
                                        receipt_id,
                                        schema_version: 1,
                                        key_id: signer.key_id().to_string(),
                                        provider_id: signer.provider_id().to_string(),
                                        request_id: r_id.clone(),
                                        session_id: session_id.as_ref().map(|id| id.0.to_string()),
                                        model,
                                        inference_status: "completed".to_string(),
                                        payment_status: "settled".to_string(),
                                        payment_hash: Some(payment_hash.0),
                                        approved_max_sats: charge_sats,
                                        actual_charge_sats: charge_sats,
                                        currency: "sats".to_string(),
                                        prompt_tokens: Some(p_tok),
                                        completion_tokens: None,
                                        error_code: None,
                                        reconciliation_id: None,
                                        supersedes_receipt_id: None,
                                        created_at: chrono::Utc::now().timestamp(),
                                    };
                                    signer.sign_receipt(payload).map(Some)
                                } else {
                                    Ok(None)
                                };
                                persist_stream_completion(
                                    db,
                                    stats,
                                    r_id,
                                    reservation_id,
                                    p_tok,
                                    charge_sats,
                                    receipt,
                                )
                                .await
                            });
                            self.finalizer = Some(fut);
                            cx.waker().wake_by_ref();
                            return Poll::Pending;
                        }

                        self.completed.store(true, Ordering::SeqCst);
                        self.done_emitted = true;
                        Poll::Ready(Some(Ok(bytes::Bytes::from("data: [DONE]\n\n"))))
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
                                        let recon_store = db.reconciliation_store();
                                        let r_id = self.req_id.clone();
                                        let session_id = self.session_id.clone();
                                        tokio::spawn(async move {
                                            if let Err(error) = req_store
                                                .mark_stream_interrupted(
                                                    &r_id,
                                                    "stream_idle_timeout",
                                                    None,
                                                )
                                                .await
                                            {
                                                tracing::error!(request_id = %r_id, %error, "Failed to record interrupted stream request");
                                            }
                                            if let Err(error) = recon_store.create_or_update_item(
                                                crate::node::storage::ReconciliationItemType::InferenceRequest,
                                                &r_id,
                                                None,
                                                session_id.as_ref(),
                                                "stream_idle_timeout",
                                                Some("Upstream stream became idle after dispatch; execution outcome is unknown"),
                                                None,
                                            ).await {
                                                tracing::error!(request_id = %r_id, %error, "Failed to persist interrupted stream reconciliation item");
                                            }
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
            session_id: session_id_for_stream,
            session_reservation_id: session_reservation_for_stream,
            model: model_for_stream,
            payment_hash: payment_hash_for_stream,
            charge_sats: charge_sats_for_stream,
            db: db_opt,
            stats: stats_for_stream,
            prompt_tokens: prompt_tokens_i64,
            completed: std::sync::Arc::new(AtomicBool::new(false)),
            saw_done_marker: false,
            terminal_marker_tail: Vec::new(),
            deadline: std::time::Instant::now() + total_timeout,
            idle_sleep: Box::pin(tokio::time::sleep(idle_timeout)),
            idle_timeout,
            error_emitted: false,
            _permit: concurrency_permit,
            finalizer: None,
            pending_emission: None,
            done_emitted: false,
        };

        // MonitoredStream has assumed ownership; defuse dispatch guard
        dispatch_guard.defuse();

        let body = axum::body::Body::from_stream(monitored_stream);
        let mut resp = body.into_response();
        resp.headers_mut().extend(response_headers);
        resp.headers_mut()
            .insert("content-type", "text/event-stream".parse().unwrap());

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

                // PayGuard 2A.3: Generate, sign, and persist immutable receipt
                if let (Some(db), Some(signer)) = (&state.db, AppState::receipt_signer()) {
                    let receipt_id = format!("rcpt_{}", uuid::Uuid::new_v4().simple());
                    let payload = crate::node::receipt::ReceiptPayload {
                        receipt_id: receipt_id.clone(),
                        schema_version: 1,
                        key_id: signer.key_id().to_string(),
                        provider_id: signer.provider_id().to_string(),
                        request_id: internal_request_id.clone(),
                        session_id: session_opt.as_ref().map(|s| s.0.to_string()),
                        model: validated.model.clone(),
                        inference_status: "completed".to_string(),
                        payment_status: "settled".to_string(),
                        payment_hash: Some(payment_hash.0.clone()),
                        approved_max_sats: cost.0,
                        actual_charge_sats: cost.0,
                        currency: "sats".to_string(),
                        prompt_tokens: prompt_tok,
                        completion_tokens: comp_tok,
                        error_code: None,
                        reconciliation_id: None,
                        supersedes_receipt_id: None,
                        created_at: chrono::Utc::now().timestamp(),
                    };
                    match signer.sign_receipt(payload) {
                        Ok(signed) => match db.receipt_store().create_receipt(&signed).await {
                            Ok(()) => match receipt_id.parse() {
                                Ok(header_val) => {
                                    response_headers.insert("x-satsgrid-receipt-id", header_val);
                                }
                                Err(err) => {
                                    tracing::error!(
                                        request_id = %internal_request_id,
                                        receipt_id = %receipt_id,
                                        error = %err,
                                        "Failed to parse receipt ID as HTTP header value"
                                    );
                                }
                            },
                            Err(err) => {
                                tracing::error!(
                                    request_id = %internal_request_id,
                                    receipt_id = %receipt_id,
                                    error = %err,
                                    "Failed to persist signed receipt for completed non-streaming request"
                                );
                            }
                        },
                        Err(err) => {
                            tracing::error!(
                                request_id = %internal_request_id,
                                error = %err,
                                "Failed to sign receipt for completed non-streaming request"
                            );
                        }
                    }
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

    let (
        requests,
        earned,
        active_sessions,
        reserved_sats,
        completed_reqs,
        failed_reqs,
        recon_reqs,
        model_breakdown_data,
        recent_reqs_data,
        pending_inv,
        settled_inv,
        expired_inv,
        cancelled_inv,
        unresolved_recon,
        worker_stats,
    ) = if let Some(db) = &state.db {
        let inv_store = db.invoice_store();
        let recon_store = db.reconciliation_store();
        let req_store = db.request_store();
        let session_store = db.session_store();

        let total_reqs = req_store.count_completed_total().await.unwrap_or_else(|_| {
            state
                .stats
                .total_requests
                .load(std::sync::atomic::Ordering::SeqCst)
        });
        let total_earned = req_store
            .total_completed_revenue_sats()
            .await
            .unwrap_or_else(|_| {
                state
                    .stats
                    .total_sats_earned
                    .load(std::sync::atomic::Ordering::SeqCst)
            });
        let active_sess = session_store.count_active_sessions().await.unwrap_or(0);
        let total_reserved = session_store.total_reserved_sats().await.unwrap_or(0);

        let completed_c = total_reqs;
        let failed_c = req_store.count_by_status("failed").await.unwrap_or(0);
        let recon_c = req_store.count_by_status("reconciliation_required").await.unwrap_or(0);

        let models = req_store.model_activity_breakdown().await.unwrap_or_default();
        let recent = req_store.fetch_recent_requests(10).await.unwrap_or_default();

        let p = inv_store
            .count_by_status(crate::node::storage::InvoiceStatus::Pending)
            .await
            .unwrap_or(0);
        let s = inv_store
            .count_by_status(crate::node::storage::InvoiceStatus::Settled)
            .await
            .unwrap_or(0);
        let exp = inv_store
            .count_by_status(crate::node::storage::InvoiceStatus::Expired)
            .await
            .unwrap_or(0);
        let canc = inv_store
            .count_by_status(crate::node::storage::InvoiceStatus::Cancelled)
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
        (
            total_reqs,
            total_earned,
            active_sess,
            total_reserved,
            completed_c,
            failed_c,
            recon_c,
            models,
            recent,
            p,
            s,
            exp,
            canc,
            u,
            ws,
        )
    } else {
        let requests = state
            .stats
            .total_requests
            .load(std::sync::atomic::Ordering::SeqCst);
        let earned = state
            .stats
            .total_sats_earned
            .load(std::sync::atomic::Ordering::SeqCst);
        (requests, earned, 0, 0, requests, 0, 0, Vec::new(), Vec::new(), 0, 0, 0, 0, 0, None)
    };

    let model_breakdown_json: Vec<_> = model_breakdown_data
        .into_iter()
        .map(|(model, cnt, rev)| {
            json!({
                "model": model,
                "completed_requests": cnt,
                "sats_earned": rev,
            })
        })
        .collect();

    let recent_requests_json: Vec<_> = recent_reqs_data
        .into_iter()
        .map(|(id, model, status, charged_sats, created_at)| {
            json!({
                "id": id,
                "model": model,
                "status": status,
                "charged_sats": charged_sats,
                "created_at": created_at,
            })
        })
        .collect();

    Ok(Json(json!({
        "total_requests": requests,
        "total_sats_earned": earned,
        "active_sessions": active_sessions,
        "reserved_sats": reserved_sats,
        "requests_breakdown": {
            "completed": completed_reqs,
            "failed": failed_reqs,
            "reconciliation_required": recon_reqs,
        },
        "model_breakdown": model_breakdown_json,
        "recent_requests": recent_requests_json,
        "invoices": {
            "pending": pending_inv,
            "settled": settled_inv,
            "expired": expired_inv,
            "cancelled": cancelled_inv,
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

    Ok(Json(json!({
        "server": {
            "host": state.config.server.host,
            "port": state.config.server.port,
        },
        "lightning": {
            "backend": backend_str,
            "network": network_str,
        },
        "pricing": {
            "default_price_sats": pricing.default_price_sats.0,
            "flat_rate_sats": pricing.default_price_sats.0,
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
            Error::NotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
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

pub async fn get_receipt(
    State(state): State<AppState>,
    axum::extract::Path(receipt_id): axum::extract::Path<String>,
) -> Result<impl IntoResponse, Error> {
    let db = state.db.as_ref().ok_or_else(|| {
        Error::Internal("Database persistence not enabled on this node".to_string())
    })?;

    let receipt = db
        .receipt_store()
        .get_receipt(&receipt_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("Receipt '{}' not found", receipt_id)))?;

    Ok(Json(receipt))
}

#[derive(serde::Deserialize)]
pub struct ReconciliationQuery {
    pub status: Option<String>,
    pub limit: Option<i64>,
}

pub async fn list_reconciliation_items(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<ReconciliationQuery>,
) -> Result<impl IntoResponse, Error> {
    state.verify_admin(&headers)?;

    let db = state.db.as_ref().ok_or_else(|| {
        Error::Internal("Database persistence not enabled on this node".to_string())
    })?;

    let status_filter = match query.status.as_deref() {
        Some(s) => Some(
            s.parse::<crate::node::storage::ReconciliationStatus>()
                .map_err(|_| {
                    Error::Validation(
                        crate::node::api::validation::OpenAiErrorResponse::invalid_request(
                            format!("Invalid reconciliation status: '{}'", s),
                            Some("status"),
                            Some("invalid_status"),
                        ),
                    )
                })?,
        ),
        None => None,
    };

    let limit = query.limit.unwrap_or(50).clamp(1, 200);

    let items = db
        .reconciliation_store()
        .list_items(status_filter, limit)
        .await?;

    let sanitized_items: Vec<serde_json::Value> = items
        .into_iter()
        .map(|item| {
            json!({
                "id": item.id,
                "item_type": item.item_type.as_str(),
                "reference_id": item.reference_id,
                "payment_hash": item.payment_hash.map(|h| h.0),
                "session_id": item.session_id.map(|s| s.0.to_string()),
                "status": item.status.as_str(),
                "reason": item.reason,
                "error_details": item.error_details,
                "attempt_count": item.attempt_count,
                "first_detected_at": item.first_detected_at,
                "last_attempt_at": item.last_attempt_at,
                "next_retry_at": item.next_retry_at,
                "resolved_at": item.resolved_at,
                "reserved_sats": item.reserved_sats,
            })
        })
        .collect();

    Ok(Json(json!({
        "items": sanitized_items,
        "count": sanitized_items.len(),
    })))
}

#[cfg(test)]
mod streaming_finalization_tests {
    use super::*;
    use crate::common::types::{Satoshis, SessionId};
    use crate::node::storage::{Database, ReconciliationStatus, RequestMode, RequestStatus};

    #[tokio::test]
    async fn receipt_signing_failure_moves_processing_request_to_recovery() {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let db = std::sync::Arc::new(Database::init(directory.path()).await.expect("database"));
        let request_id = "receipt-signing-failure-test";
        let session_id = SessionId(uuid::Uuid::new_v4());
        db.session_store()
            .create_session(
                &session_id,
                None,
                Satoshis(10),
                chrono::Utc::now().timestamp() + 3600,
            )
            .await
            .expect("create test session");
        let requests = db.request_store();
        requests
            .create_request(
                request_id,
                None,
                Some(&session_id),
                None,
                None,
                "test-model",
                RequestMode::Streaming,
                Satoshis(10),
            )
            .await
            .expect("create request");
        requests
            .mark_processing(request_id)
            .await
            .expect("mark request processing");

        let stats = std::sync::Arc::new(super::super::NodeStats::default());
        persist_stream_completion(
            db.clone(),
            stats.clone(),
            request_id.to_string(),
            None,
            1,
            10,
            Err(Error::Internal("injected signing failure".to_string())),
        )
        .await;

        let request = requests
            .get_request(request_id)
            .await
            .expect("read request")
            .expect("request exists");
        assert_eq!(request.status, RequestStatus::ReconciliationRequired);
        assert_eq!(
            request.error_code.as_deref(),
            Some("stream_receipt_signing_failed")
        );
        let items = db
            .reconciliation_store()
            .list_items(Some(ReconciliationStatus::Unresolved), 10)
            .await
            .expect("read reconciliation items");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].reference_id, request_id);
        assert_eq!(items[0].reason, "stream_receipt_signing_failed");
        assert_eq!(
            stats
                .total_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            stats
                .total_sats_earned
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }
}
