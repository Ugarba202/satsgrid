use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use satsgrid::config::schema::{NodeConfig, PricingConfig};
use satsgrid::node::api::routes::create_routes;
use satsgrid::node::api::AppState;
use satsgrid::node::gate::challenge::L402Challenge;
use satsgrid::node::gate::{MacaroonService, SessionBudgetManager};
use satsgrid::node::lightning::mock::MockLightningBackend;
use satsgrid::node::proxy::openai::OpenAiProxy;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt; // for `oneshot`
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn setup_app() -> axum::Router {
    let config = NodeConfig {
        server: satsgrid::config::schema::ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            admin_token: None,
        },
        pricing: PricingConfig::new(satsgrid::common::types::Satoshis(10)),
        upstream: satsgrid::config::schema::UpstreamConfig {
            url: "http://localhost:8080".to_string(),
        },
        lightning: satsgrid::config::schema::LightningConfig::default(),
        data_dir: ".satsgrid_test_data".to_string(),
        ..Default::default()
    };

    let proxy = OpenAiProxy::new(config.upstream.url.clone());

    let state = AppState {
        config: std::sync::Arc::new(config.clone()),
        live_pricing: std::sync::Arc::new(tokio::sync::RwLock::new(config.pricing.clone())),
        lightning: Arc::new(MockLightningBackend::new()),
        budget_manager: Arc::new(SessionBudgetManager::new()),
        macaroon_service: Arc::new(MacaroonService::new(
            b"test-secret-key-0000000000000000".to_vec(),
            "satsgrid-node",
        )),
        proxy,
        stats: Arc::new(satsgrid::node::api::NodeStats::default()),
        admin_token: Arc::new("test-admin-token".to_string()),
    };

    create_routes(state)
}

#[tokio::test]
async fn test_health_endpoint() {
    let app = setup_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_models_endpoint() {
    let app = setup_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_models_endpoint_dynamic_upstream() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "object": "list",
            "data": [
                {
                    "id": "deepseek-coder:6.7b",
                    "object": "model",
                    "created": 1700000000,
                    "owned_by": "ollama"
                }
            ]
        })))
        .mount(&mock_server)
        .await;

    let config = NodeConfig {
        server: satsgrid::config::schema::ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            admin_token: None,
        },
        pricing: PricingConfig::new(satsgrid::common::types::Satoshis(10)),
        upstream: satsgrid::config::schema::UpstreamConfig {
            url: mock_server.uri(),
        },
        lightning: satsgrid::config::schema::LightningConfig::default(),
        data_dir: ".satsgrid_test_data".to_string(),
        ..Default::default()
    };

    let proxy = OpenAiProxy::new(config.upstream.url.clone());
    let lightning = Arc::new(MockLightningBackend::new());
    let budget_manager = Arc::new(SessionBudgetManager::new());
    let macaroon_service = Arc::new(MacaroonService::new(vec![0u8; 32], "satsgrid-node"));

    let state = AppState {
        config: std::sync::Arc::new(config.clone()),
        live_pricing: std::sync::Arc::new(tokio::sync::RwLock::new(config.pricing.clone())),
        lightning,
        budget_manager,
        macaroon_service,
        proxy,
        stats: Arc::new(satsgrid::node::api::NodeStats::default()),
        admin_token: Arc::new("test-admin-token".to_string()),
    };

    let app = create_routes(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body_json["data"][0]["id"], "deepseek-coder:6.7b");
}

#[tokio::test]
async fn test_chat_completions_requires_payment() {
    let app = setup_app();

    let req_body = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "Hello"}]
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&req_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // A missing Authorization header must trigger a 402 Payment Required with an L402 challenge
    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

    // It MUST return a standard WWW-Authenticate L402 challenge header with token and invoice
    let auth_header = response
        .headers()
        .get("WWW-Authenticate")
        .expect("Must return WWW-Authenticate header on 402");
    let challenge_str = auth_header.to_str().unwrap();
    assert!(challenge_str.starts_with("L402 "));
    let challenge = L402Challenge::from_header_value(challenge_str)
        .expect("Challenge header must be valid L402 format");
    assert!(!challenge.macaroon.is_empty());
    assert!(!challenge.invoice.is_empty());
}

#[tokio::test]
async fn test_chat_completions_pay_per_request_flow() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-test",
            "choices": [{"message": {"role": "assistant", "content": "Hello! I am Infernos."}}]
        })))
        .mount(&mock_server)
        .await;

    let config = NodeConfig {
        server: satsgrid::config::schema::ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            admin_token: None,
        },
        pricing: PricingConfig::new(satsgrid::common::types::Satoshis(10)),
        upstream: satsgrid::config::schema::UpstreamConfig {
            url: mock_server.uri(),
        },
        lightning: satsgrid::config::schema::LightningConfig::default(),
        data_dir: ".satsgrid_test_data".to_string(),
        ..Default::default()
    };

    let proxy = OpenAiProxy::new(config.upstream.url.clone());
    let lightning = Arc::new(MockLightningBackend::new());
    let budget_manager = Arc::new(SessionBudgetManager::new());
    let macaroon_service = Arc::new(MacaroonService::new(
        b"test-secret-key-0000000000000000".to_vec(),
        "satsgrid-node",
    ));

    let state = AppState {
        config: std::sync::Arc::new(config.clone()),
        live_pricing: std::sync::Arc::new(tokio::sync::RwLock::new(config.pricing.clone())),
        lightning: lightning.clone(),
        budget_manager,
        macaroon_service: macaroon_service.clone(),
        proxy,
        stats: Arc::new(satsgrid::node::api::NodeStats::default()),
        admin_token: Arc::new("test-admin-token".to_string()),
    };

    let app = create_routes(state);

    // 1. Initial unauthenticated request -> returns 402 challenge
    let chat_body = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "Hello"}]
    });

    let resp_unauth = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&chat_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_unauth.status(), StatusCode::PAYMENT_REQUIRED);
    let auth_header = resp_unauth
        .headers()
        .get("WWW-Authenticate")
        .expect("Challenge header required");
    let challenge = L402Challenge::from_header_value(auth_header.to_str().unwrap()).unwrap();
    assert!(!challenge.invoice.is_empty());

    // 2. Client pays invoice (derive known preimage & payment hash for mock backend)
    let preimage_bytes = [0x77u8; 32];
    let preimage_hex = hex::encode(preimage_bytes);
    let known_payment_hash =
        satsgrid::node::gate::verify::L402Verifier::hash_preimage(&preimage_hex).unwrap();
    lightning.simulate_payment(&known_payment_hash).await;

    // 3. Mint pay-per-request macaroon bound to this payment hash (NO session caveat!)
    let ppr_macaroon = macaroon_service
        .mint(
            &known_payment_hash,
            vec![satsgrid::node::gate::macaroon::Caveat::Capability(
                "inference".to_string(),
            )],
        )
        .unwrap();

    let auth_val = format!(
        "L402 {}:{}",
        ppr_macaroon.to_base64().unwrap(),
        preimage_hex
    );

    // 4. Retry request with Authorization header -> should succeed with 200 OK!
    let resp_auth = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("Content-Type", "application/json")
                .header("Authorization", auth_val)
                .body(Body::from(serde_json::to_vec(&chat_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp_auth.status(),
        StatusCode::OK,
        "Pay-per-request should succeed without a session caveat"
    );
    let charged_header = resp_auth.headers().get("X-SatsGrid-Charged-Sats");
    assert!(charged_header.is_some());
    assert_eq!(charged_header.unwrap().to_str().unwrap(), "10");
    // Should NOT have session budget remaining header because it is a direct pay-per-request
    assert!(resp_auth
        .headers()
        .get("X-SatsGrid-Remaining-Budget-Sats")
        .is_none());
}

#[tokio::test]
async fn test_new_session_endpoint_returns_challenge() {
    let app = setup_app();

    let req_body = json!({
        "budget_sats": 1000
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/session/new")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&req_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // The endpoint should initially return a 402 challenge with the required invoice for the budget
    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

    let auth_header = response.headers().get("WWW-Authenticate");
    assert!(auth_header.is_some());
}

#[tokio::test]
async fn test_api_e2e_flow_with_budget_debit() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"response": "ok"})))
        .mount(&mock_server)
        .await;

    let config = NodeConfig {
        server: satsgrid::config::schema::ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            admin_token: None,
        },
        pricing: PricingConfig::new(satsgrid::common::types::Satoshis(10)),
        upstream: satsgrid::config::schema::UpstreamConfig {
            url: mock_server.uri(),
        },
        lightning: satsgrid::config::schema::LightningConfig::default(),
        data_dir: ".satsgrid_test_data".to_string(),
        ..Default::default()
    };

    let proxy = OpenAiProxy::new(config.upstream.url.clone());
    let lightning = Arc::new(MockLightningBackend::new());

    let state = AppState {
        config: std::sync::Arc::new(config.clone()),
        live_pricing: std::sync::Arc::new(tokio::sync::RwLock::new(config.pricing.clone())),
        lightning: lightning.clone(),
        budget_manager: Arc::new(SessionBudgetManager::new()),
        macaroon_service: Arc::new(MacaroonService::new(
            b"test-secret-key-0000000000000000".to_vec(),
            "satsgrid-node",
        )),
        proxy,
        stats: Arc::new(satsgrid::node::api::NodeStats::default()),
        admin_token: Arc::new("test-admin-token".to_string()),
    };

    let macaroon_service = state.macaroon_service.clone();
    let app = create_routes(state);

    // 1. Session New
    let req_body = json!({
        "budget_sats": 500
    });
    let response1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/session/new")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&req_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response1.status(), StatusCode::PAYMENT_REQUIRED);
    let auth_header = response1
        .headers()
        .get("WWW-Authenticate")
        .unwrap()
        .to_str()
        .unwrap();
    let challenge = L402Challenge::from_header_value(auth_header).unwrap();

    // 2. Because MockLightningBackend creates a UUID payment hash, we cannot find its preimage.
    // To test the API natively, we extract the session ID from the challenge,
    // generate a known preimage/hash pair, settle it, and re-mint a valid macaroon.
    let preimage_bytes = [0x5au8; 32];
    let preimage_hex = hex::encode(preimage_bytes);
    let known_payment_hash =
        satsgrid::node::gate::verify::L402Verifier::hash_preimage(&preimage_hex).unwrap();

    lightning.simulate_payment(&known_payment_hash).await;

    // Extract the original session to keep the flow valid
    let decoded_macaroon =
        satsgrid::node::gate::macaroon::Macaroon::from_base64(&challenge.macaroon).unwrap();
    let (session_opt, budget_opt) =
        satsgrid::node::gate::budget::SessionBudgetManager::extract_session_caveats(
            &decoded_macaroon,
        );
    let session_uuid = session_opt.unwrap().0.to_string();
    let budget_val = budget_opt.unwrap().0;

    // Remint valid macaroon
    let macaroon_obj = macaroon_service
        .mint(
            &known_payment_hash,
            vec![
                satsgrid::node::gate::macaroon::Caveat::Session(session_uuid),
                satsgrid::node::gate::macaroon::Caveat::Budget(budget_val),
                satsgrid::node::gate::macaroon::Caveat::Capability("inference".to_string()),
            ],
        )
        .unwrap();

    let auth_val = format!(
        "L402 {}:{}",
        macaroon_obj.to_base64().unwrap(),
        preimage_hex
    );

    // 4. Send chat_completions
    let chat_body = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "Hello"}]
    });

    let response2 = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("Content-Type", "application/json")
                .header("Authorization", auth_val)
                .body(Body::from(serde_json::to_vec(&chat_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response2.status();
    // With wiremock, it should be OK
    assert_eq!(status, StatusCode::OK, "Unexpected status: {}", status);

    let remaining_header = response2.headers().get("X-SatsGrid-Remaining-Budget-Sats");
    assert!(remaining_header.is_some());
    assert_eq!(remaining_header.unwrap().to_str().unwrap(), "490"); // 500 - 10
}

#[tokio::test]
async fn test_chat_completions_capability_authorization() {
    let config = NodeConfig {
        server: satsgrid::config::schema::ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            admin_token: None,
        },
        pricing: PricingConfig::new(satsgrid::common::types::Satoshis(10)),
        upstream: satsgrid::config::schema::UpstreamConfig {
            url: "http://localhost:8080".to_string(),
        },
        lightning: satsgrid::config::schema::LightningConfig::default(),
        data_dir: ".satsgrid_test_data".to_string(),
        ..Default::default()
    };

    let proxy = OpenAiProxy::new(config.upstream.url.clone());
    let lightning = Arc::new(MockLightningBackend::new());
    let budget_manager = Arc::new(SessionBudgetManager::new());

    let state = AppState {
        config: std::sync::Arc::new(config.clone()),
        live_pricing: std::sync::Arc::new(tokio::sync::RwLock::new(config.pricing.clone())),
        lightning: lightning.clone(),
        budget_manager: budget_manager.clone(),
        macaroon_service: Arc::new(MacaroonService::new(
            b"test-secret-key-0000000000000000".to_vec(),
            "satsgrid-node",
        )),
        proxy,
        stats: Arc::new(satsgrid::node::api::NodeStats::default()),
        admin_token: Arc::new("test-admin-token".to_string()),
    };

    let macaroon_service = state.macaroon_service.clone();
    let app = create_routes(state);

    let preimage_bytes = [0x5au8; 32];
    let preimage_hex = hex::encode(preimage_bytes);
    let known_payment_hash =
        satsgrid::node::gate::verify::L402Verifier::hash_preimage(&preimage_hex).unwrap();
    lightning.simulate_payment(&known_payment_hash).await;

    let chat_body = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "Hello"}]
    });

    let send_req = |macaroon_obj: satsgrid::node::gate::macaroon::Macaroon,
                    app: axum::Router,
                    preimage: String,
                    body_val: serde_json::Value| async move {
        let auth_val = format!("L402 {}:{}", macaroon_obj.to_base64().unwrap(), preimage);

        app.oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("Content-Type", "application/json")
                .header("Authorization", auth_val)
                .body(Body::from(serde_json::to_vec(&body_val).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap()
    };

    let session_uuid_val = uuid::Uuid::new_v4();
    let session_uuid = session_uuid_val.to_string();
    budget_manager
        .open_session(
            &satsgrid::common::types::SessionId(session_uuid_val),
            satsgrid::common::types::Satoshis(100),
        )
        .await;

    // 1. Missing capability rejected
    let macaroon_missing = macaroon_service
        .mint(
            &known_payment_hash,
            vec![
                satsgrid::node::gate::macaroon::Caveat::Session(session_uuid.clone()),
                satsgrid::node::gate::macaroon::Caveat::Budget(100),
            ],
        )
        .unwrap();
    let resp_missing = send_req(
        macaroon_missing,
        app.clone(),
        preimage_hex.clone(),
        chat_body.clone(),
    )
    .await;
    assert_eq!(resp_missing.status(), StatusCode::FORBIDDEN);

    // 2. Wrong capability rejected
    let macaroon_wrong = macaroon_service
        .mint(
            &known_payment_hash,
            vec![
                satsgrid::node::gate::macaroon::Caveat::Session(session_uuid.clone()),
                satsgrid::node::gate::macaroon::Caveat::Budget(100),
                satsgrid::node::gate::macaroon::Caveat::Capability("compute".to_string()),
            ],
        )
        .unwrap();
    let resp_wrong = send_req(
        macaroon_wrong,
        app.clone(),
        preimage_hex.clone(),
        chat_body.clone(),
    )
    .await;
    assert_eq!(resp_wrong.status(), StatusCode::FORBIDDEN);

    // 3. Tampered capability rejected
    let mut macaroon_tampered = macaroon_service
        .mint(
            &known_payment_hash,
            vec![
                satsgrid::node::gate::macaroon::Caveat::Session(session_uuid.clone()),
                satsgrid::node::gate::macaroon::Caveat::Budget(100),
                satsgrid::node::gate::macaroon::Caveat::Capability("compute".to_string()),
            ],
        )
        .unwrap();
    macaroon_tampered.caveats.pop();
    macaroon_tampered
        .caveats
        .push("capability = inference".to_string());
    let resp_tampered = send_req(
        macaroon_tampered,
        app.clone(),
        preimage_hex.clone(),
        chat_body.clone(),
    )
    .await;
    assert_eq!(resp_tampered.status(), StatusCode::UNAUTHORIZED);

    // 4. Capability=inference accepted
    let macaroon_valid = macaroon_service
        .mint(
            &known_payment_hash,
            vec![
                satsgrid::node::gate::macaroon::Caveat::Session(session_uuid.clone()),
                satsgrid::node::gate::macaroon::Caveat::Budget(100),
                satsgrid::node::gate::macaroon::Caveat::Capability("inference".to_string()),
            ],
        )
        .unwrap();
    let resp_valid = send_req(
        macaroon_valid,
        app.clone(),
        preimage_hex.clone(),
        chat_body.clone(),
    )
    .await;
    // Auth succeeded, fails at proxy layer
    assert_eq!(resp_valid.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn test_node_stats_security_access_control() {
    let app = setup_app();

    // 1. Missing Authorization header -> 401 Unauthorized
    let resp_unauth = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/node/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_unauth.status(), StatusCode::UNAUTHORIZED);

    // 2. Invalid Bearer token -> 401 Unauthorized
    let resp_bad_token = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/node/stats")
                .header("Authorization", "Bearer invalid_secret_token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_bad_token.status(), StatusCode::UNAUTHORIZED);

    // 3. Valid admin token -> 200 OK
    let resp_auth = app
        .oneshot(
            Request::builder()
                .uri("/v1/node/stats")
                .header("Authorization", "Bearer test-admin-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_auth.status(), StatusCode::OK);
    let body_bytes = axum::body::to_bytes(resp_auth.into_body(), usize::MAX)
        .await
        .unwrap();
    let stats_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert!(stats_json.get("total_requests").is_some());
    assert!(stats_json.get("total_sats_earned").is_some());
}

#[tokio::test]
async fn test_node_config_security_access_control() {
    let app = setup_app();

    // 1. Missing Authorization header on GET -> 401 Unauthorized
    let resp_get_unauth = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/node/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_get_unauth.status(), StatusCode::UNAUTHORIZED);

    // 2. Missing Authorization header on POST -> 401 Unauthorized (prevents remote price tampering)
    let payload = json!({
        "default_price_sats": 0,
        "sats_per_prompt_token": 0,
        "sats_per_completion_token": 0
    });

    let resp_post_unauth = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/node/config")
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_post_unauth.status(), StatusCode::UNAUTHORIZED);

    // 3. Invalid admin token -> 401 Unauthorized
    let resp_post_bad_token = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/node/config")
                .header("Authorization", "Bearer hacker_token")
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_post_bad_token.status(), StatusCode::UNAUTHORIZED);

    // 4. Authorized POST with valid admin token -> 200 OK and pricing updated
    let valid_payload = json!({
        "default_price_sats": 42,
        "sats_per_prompt_token": 1,
        "sats_per_completion_token": 2
    });

    let resp_post_auth = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/node/config")
                .header("Authorization", "Bearer test-admin-token")
                .header("content-type", "application/json")
                .body(Body::from(valid_payload.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_post_auth.status(), StatusCode::OK);

    // 5. Authorized GET confirms updated pricing
    let resp_get_auth = app
        .oneshot(
            Request::builder()
                .uri("/v1/node/config")
                .header("Authorization", "Bearer test-admin-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp_get_auth.status(), StatusCode::OK);
    let body_bytes = axum::body::to_bytes(resp_get_auth.into_body(), usize::MAX)
        .await
        .unwrap();
    let config_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(config_json["pricing"]["default_price_sats"], 42);
}
