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
    Json(payload): Json<NewSessionRequest>,
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

    // Register session in the budget manager
    state.budget_manager.open_session(&session_id, budget).await;

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
    Json(payload): Json<Value>,
) -> Result<impl IntoResponse, Error> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let prompt_tokens =
        crate::node::pricing::PricingCalculator::estimate_prompt_tokens_from_payload(&payload);
    let max_completion_tokens = payload
        .get("max_tokens")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let cost = state
        .pricing_calculator()
        .await
        .calculate_cost(prompt_tokens, max_completion_tokens);
    let req_model = payload.get("model").and_then(|m| m.as_str());

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

    if let Some(session_id) = session_opt {
        let remaining = state
            .budget_manager
            .debit_session(&session_id, cost)
            .await
            .map_err(|e| Error::BudgetExhausted(e.to_string()))?;

        response_headers.insert(
            "X-SatsGrid-Remaining-Budget-Sats",
            remaining.0.to_string().parse().unwrap(),
        );
    }

    let is_stream = payload
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);

    if is_stream {
        let stream = state.proxy.stream_chat_completion(payload).await?;
        let body = axum::body::Body::from_stream(stream);
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
        // Proxy the request
        let proxy_resp = state
            .proxy
            .forward_chat_completion_with_headers(payload, headers)
            .await?;

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
