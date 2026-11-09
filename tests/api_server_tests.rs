use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use satsgrid::config::schema::{NodeConfig, PricingConfig};
use satsgrid::node::api::routes::create_routes;
use satsgrid::node::api::AppState;
use satsgrid::node::gate::challenge::L402Challenge;
use satsgrid::node::gate::macaroon::Macaroon;
use satsgrid::node::gate::{MacaroonService, SessionBudgetManager};
use satsgrid::node::lightning::mock::MockLightningBackend;
use satsgrid::node::proxy::openai::OpenAiProxy;
use satsgrid::node::storage::Database;
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
            ..Default::default()
        },
        pricing: PricingConfig::new(satsgrid::common::types::Satoshis(10)),
        upstream: satsgrid::config::schema::UpstreamConfig {
            url: "http://localhost:8080".to_string(),
            ..Default::default()
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
        db: None,
        concurrency_manager: Arc::new(satsgrid::node::concurrency::ConcurrencyManager::new(
            config.concurrency.max_concurrent_requests,
            config.concurrency.max_concurrent_per_session,
            config.concurrency.admission_timeout_secs,
        )),
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
            ..Default::default()
        },
        pricing: PricingConfig::new(satsgrid::common::types::Satoshis(10)),
        upstream: satsgrid::config::schema::UpstreamConfig {
            url: mock_server.uri(),
            ..Default::default()
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
        db: None,
        concurrency_manager: Arc::new(satsgrid::node::concurrency::ConcurrencyManager::new(
            config.concurrency.max_concurrent_requests,
            config.concurrency.max_concurrent_per_session,
            config.concurrency.admission_timeout_secs,
        )),
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
            ..Default::default()
        },
        pricing: PricingConfig::new(satsgrid::common::types::Satoshis(10)),
        upstream: satsgrid::config::schema::UpstreamConfig {
            url: mock_server.uri(),
            ..Default::default()
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
        db: None,
        concurrency_manager: Arc::new(satsgrid::node::concurrency::ConcurrencyManager::new(
            config.concurrency.max_concurrent_requests,
            config.concurrency.max_concurrent_per_session,
            config.concurrency.admission_timeout_secs,
        )),
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
            ..Default::default()
        },
        pricing: PricingConfig::new(satsgrid::common::types::Satoshis(10)),
        upstream: satsgrid::config::schema::UpstreamConfig {
            url: mock_server.uri(),
            ..Default::default()
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
        db: None,
        concurrency_manager: Arc::new(satsgrid::node::concurrency::ConcurrencyManager::new(
            config.concurrency.max_concurrent_requests,
            config.concurrency.max_concurrent_per_session,
            config.concurrency.admission_timeout_secs,
        )),
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
            ..Default::default()
        },
        pricing: PricingConfig::new(satsgrid::common::types::Satoshis(10)),
        upstream: satsgrid::config::schema::UpstreamConfig {
            url: "http://localhost:8080".to_string(),
            ..Default::default()
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
        db: None,
        concurrency_manager: Arc::new(satsgrid::node::concurrency::ConcurrencyManager::new(
            config.concurrency.max_concurrent_requests,
            config.concurrency.max_concurrent_per_session,
            config.concurrency.admission_timeout_secs,
        )),
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

// =========================================================================
// Step 2 Regression Tests: Lazy Session Activation Lifecycle (Tests A - H)
// =========================================================================

#[tokio::test]
async fn test_step2_test_a_unpaid_session_challenge() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");

    let mock_server = MockServer::start().await;
    let config = NodeConfig {
        server: satsgrid::config::schema::ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            admin_token: None,
            ..Default::default()
        },
        pricing: PricingConfig::new(satsgrid::common::types::Satoshis(10)),
        upstream: satsgrid::config::schema::UpstreamConfig {
            url: mock_server.uri(),
            ..Default::default()
        },
        lightning: satsgrid::config::schema::LightningConfig::default(),
        data_dir: dir.path().to_str().unwrap().to_string(),
        ..Default::default()
    };

    let proxy = OpenAiProxy::new(config.upstream.url.clone());
    let lightning = Arc::new(MockLightningBackend::new());
    let budget_manager = Arc::new(SessionBudgetManager::with_store(db.session_store()));
    let macaroon_service = Arc::new(MacaroonService::new(vec![1u8; 32], "satsgrid-node"));

    let state = AppState {
        config: std::sync::Arc::new(config.clone()),
        live_pricing: std::sync::Arc::new(tokio::sync::RwLock::new(config.pricing.clone())),
        lightning,
        budget_manager,
        macaroon_service,
        proxy,
        stats: Arc::new(satsgrid::node::api::NodeStats::default()),
        admin_token: Arc::new("test-token".to_string()),
        db: Some(Arc::new(db.clone())),
        concurrency_manager: Arc::new(satsgrid::node::concurrency::ConcurrencyManager::new(
            config.concurrency.max_concurrent_requests,
            config.concurrency.max_concurrent_per_session,
            config.concurrency.admission_timeout_secs,
        )),
    };

    let app = create_routes(state);

    let req_body = json!({ "budget_sats": 500 });
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

    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);
    let auth_header = response
        .headers()
        .get("WWW-Authenticate")
        .unwrap()
        .to_str()
        .unwrap();
    let challenge = L402Challenge::from_header_value(auth_header).unwrap();
    let decoded = Macaroon::from_base64(&challenge.macaroon).unwrap();
    let (session_opt, budget_opt) = SessionBudgetManager::extract_session_caveats(&decoded);
    let session_id = session_opt.unwrap();
    assert_eq!(budget_opt, Some(satsgrid::common::types::Satoshis(500)));

    // Inspect database: status must be pending_payment, 0 balances, 0 ledger entries
    let record = db
        .session_store()
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        record.status,
        satsgrid::node::storage::SessionStatus::PendingPayment
    );
    assert_eq!(record.remaining_sats, satsgrid::common::types::Satoshis(0));
    assert_eq!(record.reserved_sats, satsgrid::common::types::Satoshis(0));
    assert_eq!(record.spent_sats, satsgrid::common::types::Satoshis(0));
    assert_eq!(
        record.total_budget_sats,
        satsgrid::common::types::Satoshis(500)
    );

    let entries = db
        .session_store()
        .get_accounting_entries(&session_id)
        .await
        .unwrap();
    assert!(
        entries.is_empty(),
        "No session_funding entry must exist for unpaid challenge"
    );
}

#[tokio::test]
async fn test_step2_test_b_unpaid_inference_rejection() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");

    let mock_server = MockServer::start().await;
    let config = NodeConfig {
        server: satsgrid::config::schema::ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            admin_token: None,
            ..Default::default()
        },
        pricing: PricingConfig::new(satsgrid::common::types::Satoshis(10)),
        upstream: satsgrid::config::schema::UpstreamConfig {
            url: mock_server.uri(),
            ..Default::default()
        },
        lightning: satsgrid::config::schema::LightningConfig::default(),
        data_dir: dir.path().to_str().unwrap().to_string(),
        ..Default::default()
    };

    let proxy = OpenAiProxy::new(config.upstream.url.clone());
    let lightning = Arc::new(MockLightningBackend::new());
    let budget_manager = Arc::new(SessionBudgetManager::with_store(db.session_store()));
    let macaroon_service = Arc::new(MacaroonService::new(vec![2u8; 32], "satsgrid-node"));

    let state = AppState {
        config: std::sync::Arc::new(config.clone()),
        live_pricing: std::sync::Arc::new(tokio::sync::RwLock::new(config.pricing.clone())),
        lightning: lightning.clone(),
        budget_manager,
        macaroon_service: macaroon_service.clone(),
        proxy,
        stats: Arc::new(satsgrid::node::api::NodeStats::default()),
        admin_token: Arc::new("test-token".to_string()),
        db: Some(Arc::new(db.clone())),
        concurrency_manager: Arc::new(satsgrid::node::concurrency::ConcurrencyManager::new(
            config.concurrency.max_concurrent_requests,
            config.concurrency.max_concurrent_per_session,
            config.concurrency.admission_timeout_secs,
        )),
    };

    let app = create_routes(state);

    // 1. Create challenge
    let req_body = json!({ "budget_sats": 300 });
    let response = app
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

    let auth_header = response
        .headers()
        .get("WWW-Authenticate")
        .unwrap()
        .to_str()
        .unwrap();
    let challenge = L402Challenge::from_header_value(auth_header).unwrap();
    let decoded = Macaroon::from_base64(&challenge.macaroon).unwrap();
    let session_id = SessionBudgetManager::extract_session_caveats(&decoded)
        .0
        .unwrap();

    // 2. Client crafts fake preimage and attempts inference without paying the invoice
    let fake_preimage = "0000000000000000000000000000000000000000000000000000000000000001";
    let fake_auth = format!("L402 {}:{}", challenge.macaroon, fake_preimage);

    let chat_body = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "Hello"}]
    });

    let resp_infer = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("Content-Type", "application/json")
                .header("Authorization", fake_auth)
                .body(Body::from(serde_json::to_vec(&chat_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_infer.status(), StatusCode::UNAUTHORIZED);

    // Database check: session remains pending_payment with 0 entries
    let record = db
        .session_store()
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        record.status,
        satsgrid::node::storage::SessionStatus::PendingPayment
    );
    let entries = db
        .session_store()
        .get_accounting_entries(&session_id)
        .await
        .unwrap();
    assert!(
        entries.is_empty(),
        "No funding entry created for unpaid inference"
    );
}

#[tokio::test]
async fn test_step2_test_c_successful_lazy_activation_and_d_duplicate_activation() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");

    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-test",
            "choices": [{"message": {"role": "assistant", "content": "Hello from model"}}]
        })))
        .mount(&mock_server)
        .await;

    let config = NodeConfig {
        server: satsgrid::config::schema::ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            admin_token: None,
            ..Default::default()
        },
        pricing: PricingConfig::new(satsgrid::common::types::Satoshis(20)),
        upstream: satsgrid::config::schema::UpstreamConfig {
            url: mock_server.uri(),
            ..Default::default()
        },
        lightning: satsgrid::config::schema::LightningConfig::default(),
        data_dir: dir.path().to_str().unwrap().to_string(),
        ..Default::default()
    };

    let proxy = OpenAiProxy::new(config.upstream.url.clone());
    let lightning = Arc::new(MockLightningBackend::new());
    let budget_manager = Arc::new(SessionBudgetManager::with_store(db.session_store()));
    let macaroon_service = Arc::new(MacaroonService::new(vec![3u8; 32], "satsgrid-node"));

    let state = AppState {
        config: std::sync::Arc::new(config.clone()),
        live_pricing: std::sync::Arc::new(tokio::sync::RwLock::new(config.pricing.clone())),
        lightning: lightning.clone(),
        budget_manager,
        macaroon_service: macaroon_service.clone(),
        proxy,
        stats: Arc::new(satsgrid::node::api::NodeStats::default()),
        admin_token: Arc::new("test-token".to_string()),
        db: Some(Arc::new(db.clone())),
        concurrency_manager: Arc::new(satsgrid::node::concurrency::ConcurrencyManager::new(
            config.concurrency.max_concurrent_requests,
            config.concurrency.max_concurrent_per_session,
            config.concurrency.admission_timeout_secs,
        )),
    };

    let app = create_routes(state);

    // 1. Session new
    let req_body = json!({ "budget_sats": 100 });
    let response = app
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

    let auth_header = response
        .headers()
        .get("WWW-Authenticate")
        .unwrap()
        .to_str()
        .unwrap();
    let challenge = L402Challenge::from_header_value(auth_header).unwrap();
    let decoded = Macaroon::from_base64(&challenge.macaroon).unwrap();
    let session_id = SessionBudgetManager::extract_session_caveats(&decoded)
        .0
        .unwrap();

    // 2. Settle the mock invoice via lightning.pay_invoice
    let preimage = lightning.pay_invoice(&challenge.invoice).await.unwrap();
    let auth_header_val = format!("L402 {}:{}", challenge.macaroon, preimage);

    // 3. First inference request triggers lazy activation!
    let chat_body = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "Hello"}]
    });

    let resp_infer1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("Content-Type", "application/json")
                .header("Authorization", &auth_header_val)
                .body(Body::from(serde_json::to_vec(&chat_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_infer1.status(), StatusCode::OK);
    let rem_header1 = resp_infer1
        .headers()
        .get("X-SatsGrid-Remaining-Budget-Sats")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(rem_header1, "80"); // 100 - 20

    // Database check: session is active, exactly 1 funding entry
    let record1 = db
        .session_store()
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        record1.status,
        satsgrid::node::storage::SessionStatus::Active
    );
    assert_eq!(
        record1.remaining_sats,
        satsgrid::common::types::Satoshis(80)
    );

    let entries1 = db
        .session_store()
        .get_accounting_entries(&session_id)
        .await
        .unwrap();
    let funding_entries1: Vec<_> = entries1
        .iter()
        .filter(|e| e.1 == "session_funding")
        .collect();
    assert_eq!(
        funding_entries1.len(),
        1,
        "Exactly one funding entry upon activation"
    );

    // Test D: Subsequent inference request on the same active session (Duplicate Activation check)
    let resp_infer2 = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("Content-Type", "application/json")
                .header("Authorization", &auth_header_val)
                .body(Body::from(serde_json::to_vec(&chat_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_infer2.status(), StatusCode::OK);
    let rem_header2 = resp_infer2
        .headers()
        .get("X-SatsGrid-Remaining-Budget-Sats")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(rem_header2, "60"); // 80 - 20

    let entries2 = db
        .session_store()
        .get_accounting_entries(&session_id)
        .await
        .unwrap();
    let funding_entries2: Vec<_> = entries2
        .iter()
        .filter(|e| e.1 == "session_funding")
        .collect();
    assert_eq!(
        funding_entries2.len(),
        1,
        "Subsequent request must NOT create another funding entry"
    );
}

#[tokio::test]
async fn test_step2_test_e_concurrent_first_requests() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");

    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-test",
            "choices": [{"message": {"role": "assistant", "content": "Concurrent response"}}]
        })))
        .mount(&mock_server)
        .await;

    let config = NodeConfig {
        server: satsgrid::config::schema::ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            admin_token: None,
            ..Default::default()
        },
        pricing: PricingConfig::new(satsgrid::common::types::Satoshis(25)),
        upstream: satsgrid::config::schema::UpstreamConfig {
            url: mock_server.uri(),
            ..Default::default()
        },
        lightning: satsgrid::config::schema::LightningConfig::default(),
        data_dir: dir.path().to_str().unwrap().to_string(),
        ..Default::default()
    };

    let proxy = OpenAiProxy::new(config.upstream.url.clone());
    let lightning = Arc::new(MockLightningBackend::new());
    let budget_manager = Arc::new(SessionBudgetManager::with_store(db.session_store()));
    let macaroon_service = Arc::new(MacaroonService::new(vec![4u8; 32], "satsgrid-node"));

    let state = AppState {
        config: std::sync::Arc::new(config.clone()),
        live_pricing: std::sync::Arc::new(tokio::sync::RwLock::new(config.pricing.clone())),
        lightning: lightning.clone(),
        budget_manager,
        macaroon_service: macaroon_service.clone(),
        proxy,
        stats: Arc::new(satsgrid::node::api::NodeStats::default()),
        admin_token: Arc::new("test-token".to_string()),
        db: Some(Arc::new(db.clone())),
        concurrency_manager: Arc::new(satsgrid::node::concurrency::ConcurrencyManager::new(
            config.concurrency.max_concurrent_requests,
            config.concurrency.max_concurrent_per_session,
            config.concurrency.admission_timeout_secs,
        )),
    };

    let app = create_routes(state);

    // 1. Session new with 100 sats budget (can afford exactly 4 requests at 25 sats each)
    let req_body = json!({ "budget_sats": 100 });
    let response = app
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

    let auth_header = response
        .headers()
        .get("WWW-Authenticate")
        .unwrap()
        .to_str()
        .unwrap();
    let challenge = L402Challenge::from_header_value(auth_header).unwrap();
    let decoded = Macaroon::from_base64(&challenge.macaroon).unwrap();
    let session_id = SessionBudgetManager::extract_session_caveats(&decoded)
        .0
        .unwrap();

    let preimage = lightning.pay_invoice(&challenge.invoice).await.unwrap();
    let auth_header_val = format!("L402 {}:{}", challenge.macaroon, preimage);

    // 2. Fire 6 concurrent requests (4 should succeed, 2 should fail with BudgetExhausted)
    let mut handles = Vec::new();
    for i in 0..6 {
        let app_clone = app.clone();
        let auth_val_clone = auth_header_val.clone();
        handles.push(tokio::spawn(async move {
            let chat_body = json!({
                "model": "llama3.2",
                "messages": [{"role": "user", "content": format!("Hello {}", i)}]
            });
            app_clone
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/v1/chat/completions")
                        .header("Content-Type", "application/json")
                        .header("Authorization", auth_val_clone)
                        .body(Body::from(serde_json::to_vec(&chat_body).unwrap()))
                        .unwrap(),
                )
                .await
                .unwrap()
        }));
    }

    let mut ok_count = 0;
    let mut budget_exhausted_count = 0;
    for handle in handles {
        let resp = handle.await.unwrap();
        if resp.status() == StatusCode::OK {
            ok_count += 1;
        } else if resp.status() == StatusCode::PAYMENT_REQUIRED {
            budget_exhausted_count += 1;
        }
    }

    assert_eq!(
        ok_count, 4,
        "Exactly 4 requests should succeed within 100 sats budget"
    );
    assert_eq!(
        budget_exhausted_count, 2,
        "2 requests should be rejected due to budget exhaustion"
    );

    // Assert exactly 1 session_funding entry
    let entries = db
        .session_store()
        .get_accounting_entries(&session_id)
        .await
        .unwrap();
    let funding_entries: Vec<_> = entries
        .iter()
        .filter(|e| e.1 == "session_funding")
        .collect();
    assert_eq!(
        funding_entries.len(),
        1,
        "Exactly one funding entry must be recorded"
    );

    // Assert final remaining balance is 0 and spent is 100
    let record = db
        .session_store()
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.remaining_sats, satsgrid::common::types::Satoshis(0));
    assert_eq!(record.spent_sats, satsgrid::common::types::Satoshis(100));
}

#[tokio::test]
async fn test_step2_test_f_invalid_preimage_and_g_expired_session() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");

    let mock_server = MockServer::start().await;
    let config = NodeConfig {
        server: satsgrid::config::schema::ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            admin_token: None,
            ..Default::default()
        },
        pricing: PricingConfig::new(satsgrid::common::types::Satoshis(10)),
        upstream: satsgrid::config::schema::UpstreamConfig {
            url: mock_server.uri(),
            ..Default::default()
        },
        lightning: satsgrid::config::schema::LightningConfig::default(),
        data_dir: dir.path().to_str().unwrap().to_string(),
        ..Default::default()
    };

    let proxy = OpenAiProxy::new(config.upstream.url.clone());
    let lightning = Arc::new(MockLightningBackend::new());
    let budget_manager = Arc::new(SessionBudgetManager::with_store(db.session_store()));
    let macaroon_service = Arc::new(MacaroonService::new(vec![5u8; 32], "satsgrid-node"));

    let state = AppState {
        config: std::sync::Arc::new(config.clone()),
        live_pricing: std::sync::Arc::new(tokio::sync::RwLock::new(config.pricing.clone())),
        lightning: lightning.clone(),
        budget_manager,
        macaroon_service: macaroon_service.clone(),
        proxy,
        stats: Arc::new(satsgrid::node::api::NodeStats::default()),
        admin_token: Arc::new("test-token".to_string()),
        db: Some(Arc::new(db.clone())),
        concurrency_manager: Arc::new(satsgrid::node::concurrency::ConcurrencyManager::new(
            config.concurrency.max_concurrent_requests,
            config.concurrency.max_concurrent_per_session,
            config.concurrency.admission_timeout_secs,
        )),
    };

    let app = create_routes(state);

    // Create session
    let req_body = json!({ "budget_sats": 200 });
    let response = app
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

    let auth_header = response
        .headers()
        .get("WWW-Authenticate")
        .unwrap()
        .to_str()
        .unwrap();
    let challenge = L402Challenge::from_header_value(auth_header).unwrap();
    let decoded = Macaroon::from_base64(&challenge.macaroon).unwrap();
    let session_id = SessionBudgetManager::extract_session_caveats(&decoded)
        .0
        .unwrap();

    // Test F: Settle invoice, but client sends corrupted preimage
    let valid_preimage = lightning.pay_invoice(&challenge.invoice).await.unwrap();
    let corrupted_preimage = format!("bad{}", &valid_preimage[3..]);
    let bad_auth = format!("L402 {}:{}", challenge.macaroon, corrupted_preimage);

    let chat_body = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "Hello"}]
    });

    let resp_bad = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("Content-Type", "application/json")
                .header("Authorization", bad_auth)
                .body(Body::from(serde_json::to_vec(&chat_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_bad.status(), StatusCode::UNAUTHORIZED);
    let record = db
        .session_store()
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        record.status,
        satsgrid::node::storage::SessionStatus::PendingPayment,
        "Corrupted preimage must not activate session"
    );

    // Test G: Expired pending session cannot activate
    sqlx::query("UPDATE sessions SET expires_at = 1000 WHERE id = ?1")
        .bind(session_id.0.to_string())
        .execute(db.pool())
        .await
        .unwrap();

    let good_auth = format!("L402 {}:{}", challenge.macaroon, valid_preimage);
    let resp_expired = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("Content-Type", "application/json")
                .header("Authorization", good_auth)
                .body(Body::from(serde_json::to_vec(&chat_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_expired.status(), StatusCode::UNAUTHORIZED);
    let entries = db
        .session_store()
        .get_accounting_entries(&session_id)
        .await
        .unwrap();
    assert!(
        entries.is_empty(),
        "Expired session must not be credited funding"
    );
}

#[tokio::test]
async fn test_step2_test_h_restart_persistence() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db_path = dir.path().join("restart_test.db");

    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-test",
            "choices": [{"message": {"role": "assistant", "content": "Post-restart response"}}]
        })))
        .mount(&mock_server)
        .await;

    let lightning = Arc::new(MockLightningBackend::new());
    let macaroon_service = Arc::new(MacaroonService::new(vec![6u8; 32], "satsgrid-node"));
    let (challenge_invoice, challenge_macaroon, target_session_id);

    // Phase 1: Initialize node, create pending session, then drop node
    {
        let db1 = Database::init(&db_path).await.expect("init db1");
        let config1 = NodeConfig {
            server: satsgrid::config::schema::ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 8080,
                admin_token: None,
                ..Default::default()
            },
            pricing: PricingConfig::new(satsgrid::common::types::Satoshis(15)),
            upstream: satsgrid::config::schema::UpstreamConfig {
                url: mock_server.uri(),
                ..Default::default()
            },
            lightning: satsgrid::config::schema::LightningConfig::default(),
            data_dir: dir.path().to_str().unwrap().to_string(),
            ..Default::default()
        };

        let state1 = AppState {
            config: std::sync::Arc::new(config1.clone()),
            live_pricing: std::sync::Arc::new(tokio::sync::RwLock::new(config1.pricing.clone())),
            lightning: lightning.clone(),
            budget_manager: Arc::new(SessionBudgetManager::with_store(db1.session_store())),
            macaroon_service: macaroon_service.clone(),
            proxy: OpenAiProxy::new(config1.upstream.url.clone()),
            stats: Arc::new(satsgrid::node::api::NodeStats::default()),
            admin_token: Arc::new("test-token".to_string()),
            db: Some(Arc::new(db1.clone())),
            concurrency_manager: Arc::new(satsgrid::node::concurrency::ConcurrencyManager::new(
                config1.concurrency.max_concurrent_requests,
                config1.concurrency.max_concurrent_per_session,
                config1.concurrency.admission_timeout_secs,
            )),
        };

        let app1 = create_routes(state1);

        let req_body = json!({ "budget_sats": 250 });
        let resp = app1
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

        let auth_header = resp
            .headers()
            .get("WWW-Authenticate")
            .unwrap()
            .to_str()
            .unwrap();
        let challenge = L402Challenge::from_header_value(auth_header).unwrap();
        let decoded = Macaroon::from_base64(&challenge.macaroon).unwrap();
        target_session_id = SessionBudgetManager::extract_session_caveats(&decoded)
            .0
            .unwrap();
        challenge_invoice = challenge.invoice;
        challenge_macaroon = challenge.macaroon;
    }

    // Phase 2: Restart node against same persistent SQLite DB
    {
        let db2 = Database::init(&db_path).await.expect("init db2");

        // Verify session is still pending before settlement
        let record = db2
            .session_store()
            .get_session(&target_session_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            record.status,
            satsgrid::node::storage::SessionStatus::PendingPayment
        );
        assert_eq!(record.remaining_sats, satsgrid::common::types::Satoshis(0));

        // Client pays invoice
        let preimage = lightning.pay_invoice(&challenge_invoice).await.unwrap();
        let auth_val = format!("L402 {}:{}", challenge_macaroon, preimage);

        let config2 = NodeConfig {
            server: satsgrid::config::schema::ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 8080,
                admin_token: None,
                ..Default::default()
            },
            pricing: PricingConfig::new(satsgrid::common::types::Satoshis(15)),
            upstream: satsgrid::config::schema::UpstreamConfig {
                url: mock_server.uri(),
                ..Default::default()
            },
            lightning: satsgrid::config::schema::LightningConfig::default(),
            data_dir: dir.path().to_str().unwrap().to_string(),
            ..Default::default()
        };

        let state2 = AppState {
            config: std::sync::Arc::new(config2.clone()),
            live_pricing: std::sync::Arc::new(tokio::sync::RwLock::new(config2.pricing.clone())),
            lightning: lightning.clone(),
            budget_manager: Arc::new(SessionBudgetManager::with_store(db2.session_store())),
            macaroon_service: macaroon_service.clone(),
            proxy: OpenAiProxy::new(config2.upstream.url.clone()),
            stats: Arc::new(satsgrid::node::api::NodeStats::default()),
            admin_token: Arc::new("test-token".to_string()),
            db: Some(Arc::new(db2.clone())),
            concurrency_manager: Arc::new(satsgrid::node::concurrency::ConcurrencyManager::new(
                config2.concurrency.max_concurrent_requests,
                config2.concurrency.max_concurrent_per_session,
                config2.concurrency.admission_timeout_secs,
            )),
        };

        let app2 = create_routes(state2);

        let chat_body = json!({
            "model": "llama3.2",
            "messages": [{"role": "user", "content": "Hello after restart"}]
        });

        let resp2 = app2
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

        assert_eq!(resp2.status(), StatusCode::OK);
        let rem_header = resp2
            .headers()
            .get("X-SatsGrid-Remaining-Budget-Sats")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(rem_header, "235"); // 250 - 15

        let updated_record = db2
            .session_store()
            .get_session(&target_session_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            updated_record.status,
            satsgrid::node::storage::SessionStatus::Active
        );
        assert_eq!(
            updated_record.remaining_sats,
            satsgrid::common::types::Satoshis(235)
        );

        let entries = db2
            .session_store()
            .get_accounting_entries(&target_session_id)
            .await
            .unwrap();
        let funding_entries: Vec<_> = entries
            .iter()
            .filter(|e| e.1 == "session_funding")
            .collect();
        assert_eq!(funding_entries.len(), 1);
    }
}

#[tokio::test]
async fn test_step2a_payment_hash_binding_enforcement() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let store = db.session_store();

    let session_id = satsgrid::common::types::SessionId(uuid::Uuid::new_v4());
    let real_phash = satsgrid::common::types::PaymentHash(
        "1111111111111111111111111111111111111111111111111111111111111111".to_string(),
    );
    let wrong_phash = satsgrid::common::types::PaymentHash(
        "2222222222222222222222222222222222222222222222222222222222222222".to_string(),
    );
    let budget = satsgrid::common::types::Satoshis(300);
    let expires_at = chrono::Utc::now().timestamp() + 3600;

    // 1. Create pending session with real_phash
    store
        .create_pending_session(&session_id, Some(&real_phash), budget, expires_at)
        .await
        .unwrap();

    // 2. Attempt activation with mismatched payment hash -> must be rejected
    let res_mismatch = store
        .activate_session(&session_id, &wrong_phash)
        .await
        .unwrap();
    assert_eq!(
        res_mismatch,
        satsgrid::node::storage::SessionActivationResult::PaymentHashMismatch
    );

    // Assert zero funding entries created
    let entries_after_bad = store.get_accounting_entries(&session_id).await.unwrap();
    assert!(
        entries_after_bad.is_empty(),
        "Mismatched payment hash must not create funding entries"
    );

    // Assert session remains pending
    let rec_pending = store.get_session(&session_id).await.unwrap().unwrap();
    assert_eq!(
        rec_pending.status,
        satsgrid::node::storage::SessionStatus::PendingPayment
    );

    // 3. Test missing persisted payment hash
    let session_id_nohash = satsgrid::common::types::SessionId(uuid::Uuid::new_v4());
    store
        .create_pending_session(&session_id_nohash, None, budget, expires_at)
        .await
        .unwrap();
    let res_nohash = store
        .activate_session(&session_id_nohash, &real_phash)
        .await
        .unwrap();
    assert_eq!(
        res_nohash,
        satsgrid::node::storage::SessionActivationResult::PaymentHashMismatch
    );

    // 4. Activate with matching payment hash -> succeeds
    let res_match = store
        .activate_session(&session_id, &real_phash)
        .await
        .unwrap();
    assert_eq!(
        res_match,
        satsgrid::node::storage::SessionActivationResult::Activated {
            total_budget: budget
        }
    );

    let entries_after_good = store.get_accounting_entries(&session_id).await.unwrap();
    assert_eq!(entries_after_good.len(), 1);
}

#[tokio::test]
async fn test_step2a_expiration_boundary_standardization() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");
    let store = db.session_store();

    let macaroon_service = MacaroonService::new(vec![7u8; 32], "satsgrid-node");
    let dummy_hash = satsgrid::common::types::PaymentHash(
        "3333333333333333333333333333333333333333333333333333333333333333".to_string(),
    );

    // Test Macaroon verification boundary (now < expires_at vs now == expires_at vs now > expires_at)
    let expires_at_u64: u64 = 10000;
    let mac = macaroon_service
        .mint(
            &dummy_hash,
            vec![satsgrid::node::gate::macaroon::Caveat::ExpiresAt(
                expires_at_u64,
            )],
        )
        .unwrap();

    // 1. now < expires_at -> valid
    assert!(macaroon_service.verify(&mac, None, 9999).is_ok());

    // 2. now == expires_at -> expired!
    let err_exact = macaroon_service.verify(&mac, None, 10000);
    assert!(
        err_exact.is_err(),
        "now == expires_at must be rejected as expired"
    );

    // 3. now > expires_at -> expired!
    let err_after = macaroon_service.verify(&mac, None, 10001);
    assert!(
        err_after.is_err(),
        "now > expires_at must be rejected as expired"
    );

    // Test SQLite reservation boundary
    let session_id = satsgrid::common::types::SessionId(uuid::Uuid::new_v4());
    let fixed_expiry = chrono::Utc::now().timestamp() - 100; // in the past
    store
        .create_session(
            &session_id,
            Some(&dummy_hash),
            satsgrid::common::types::Satoshis(100),
            fixed_expiry,
        )
        .await
        .unwrap();

    let res_expired = store
        .reserve_budget(
            &session_id,
            satsgrid::common::types::Satoshis(10),
            "res-boundary",
        )
        .await
        .unwrap();
    assert!(matches!(
        res_expired,
        satsgrid::node::storage::SessionReservationResult::Expired { .. }
    ));
}

#[tokio::test]
async fn test_step2a_closed_session_rejected_and_balance_untouched() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");

    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "chatcmpl-test",
            "choices": [{"message": {"role": "assistant", "content": "Closed test response"}}]
        })))
        .mount(&mock_server)
        .await;

    let config = NodeConfig {
        server: satsgrid::config::schema::ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            admin_token: None,
            ..Default::default()
        },
        pricing: PricingConfig::new(satsgrid::common::types::Satoshis(20)),
        upstream: satsgrid::config::schema::UpstreamConfig {
            url: mock_server.uri(),
            ..Default::default()
        },
        lightning: satsgrid::config::schema::LightningConfig::default(),
        data_dir: dir.path().to_str().unwrap().to_string(),
        ..Default::default()
    };

    let proxy = OpenAiProxy::new(config.upstream.url.clone());
    let lightning = Arc::new(MockLightningBackend::new());
    let budget_manager = Arc::new(SessionBudgetManager::with_store(db.session_store()));
    let macaroon_service = Arc::new(MacaroonService::new(vec![8u8; 32], "satsgrid-node"));

    let state = AppState {
        config: std::sync::Arc::new(config.clone()),
        live_pricing: std::sync::Arc::new(tokio::sync::RwLock::new(config.pricing.clone())),
        lightning: lightning.clone(),
        budget_manager,
        macaroon_service: macaroon_service.clone(),
        proxy,
        stats: Arc::new(satsgrid::node::api::NodeStats::default()),
        admin_token: Arc::new("test-token".to_string()),
        db: Some(Arc::new(db.clone())),
        concurrency_manager: Arc::new(satsgrid::node::concurrency::ConcurrencyManager::new(
            config.concurrency.max_concurrent_requests,
            config.concurrency.max_concurrent_per_session,
            config.concurrency.admission_timeout_secs,
        )),
    };

    let app = create_routes(state);

    // 1. Create challenge
    let req_body = json!({ "budget_sats": 200 });
    let response = app
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

    let auth_header = response
        .headers()
        .get("WWW-Authenticate")
        .unwrap()
        .to_str()
        .unwrap();
    let challenge = L402Challenge::from_header_value(auth_header).unwrap();
    let decoded = Macaroon::from_base64(&challenge.macaroon).unwrap();
    let session_id = SessionBudgetManager::extract_session_caveats(&decoded)
        .0
        .unwrap();

    // 2. Pay invoice and activate session with first request
    let preimage = lightning.pay_invoice(&challenge.invoice).await.unwrap();
    let auth_val = format!("L402 {}:{}", challenge.macaroon, preimage);

    let chat_body = json!({
        "model": "llama3.2",
        "messages": [{"role": "user", "content": "Req 1"}]
    });

    let resp1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("Content-Type", "application/json")
                .header("Authorization", &auth_val)
                .body(Body::from(serde_json::to_vec(&chat_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp1.status(), StatusCode::OK);
    let rem_header = resp1
        .headers()
        .get("X-SatsGrid-Remaining-Budget-Sats")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(rem_header, "180"); // 200 - 20

    // Confirm 1 funding entry
    let entries1 = db
        .session_store()
        .get_accounting_entries(&session_id)
        .await
        .unwrap();
    let funding_count1 = entries1.iter().filter(|e| e.1 == "session_funding").count();
    assert_eq!(funding_count1, 1);

    // 3. Transition session to 'closed' in storage
    sqlx::query("UPDATE sessions SET status = 'closed', updated_at = unixepoch() WHERE id = ?1")
        .bind(session_id.0.to_string())
        .execute(db.pool())
        .await
        .unwrap();

    // 4. Attempt another request on the closed session
    let resp2 = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("Content-Type", "application/json")
                .header("Authorization", &auth_val)
                .body(Body::from(serde_json::to_vec(&chat_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Must be rejected
    assert_eq!(resp2.status(), StatusCode::UNAUTHORIZED);

    // Assert session remains closed, balance is untouched, and funding count remains 1
    let rec_closed = db
        .session_store()
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        rec_closed.status,
        satsgrid::node::storage::SessionStatus::Closed
    );
    assert_eq!(
        rec_closed.remaining_sats,
        satsgrid::common::types::Satoshis(180)
    );

    let entries2 = db
        .session_store()
        .get_accounting_entries(&session_id)
        .await
        .unwrap();
    let funding_count2 = entries2.iter().filter(|e| e.1 == "session_funding").count();
    assert_eq!(funding_count2, 1);
}

#[tokio::test]
async fn test_step2a_streaming_lazy_activation() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db = Database::init(dir.path()).await.expect("init db");

    let mock_server = MockServer::start().await;
    let sse_body = "data: {\"id\":\"chatcmpl-stream\",\"choices\":[{\"delta\":{\"content\":\"Hello world\"}}]}\n\ndata: [DONE]\n\n";

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(sse_body, "text/event-stream"),
        )
        .mount(&mock_server)
        .await;

    let config = NodeConfig {
        server: satsgrid::config::schema::ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            admin_token: None,
            ..Default::default()
        },
        pricing: PricingConfig::new(satsgrid::common::types::Satoshis(10)),
        upstream: satsgrid::config::schema::UpstreamConfig {
            url: mock_server.uri(),
            ..Default::default()
        },
        lightning: satsgrid::config::schema::LightningConfig::default(),
        data_dir: dir.path().to_str().unwrap().to_string(),
        ..Default::default()
    };

    let proxy = OpenAiProxy::new(config.upstream.url.clone());
    let lightning = Arc::new(MockLightningBackend::new());
    let budget_manager = Arc::new(SessionBudgetManager::with_store(db.session_store()));
    let macaroon_service = Arc::new(MacaroonService::new(vec![9u8; 32], "satsgrid-node"));

    let state = AppState {
        config: std::sync::Arc::new(config.clone()),
        live_pricing: std::sync::Arc::new(tokio::sync::RwLock::new(config.pricing.clone())),
        lightning: lightning.clone(),
        budget_manager,
        macaroon_service: macaroon_service.clone(),
        proxy,
        stats: Arc::new(satsgrid::node::api::NodeStats::default()),
        admin_token: Arc::new("test-token".to_string()),
        db: Some(Arc::new(db.clone())),
        concurrency_manager: Arc::new(satsgrid::node::concurrency::ConcurrencyManager::new(
            config.concurrency.max_concurrent_requests,
            config.concurrency.max_concurrent_per_session,
            config.concurrency.admission_timeout_secs,
        )),
    };

    let app = create_routes(state);

    // 1. Create challenge
    let req_body = json!({ "budget_sats": 150 });
    let response = app
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

    let auth_header = response
        .headers()
        .get("WWW-Authenticate")
        .unwrap()
        .to_str()
        .unwrap();
    let challenge = L402Challenge::from_header_value(auth_header).unwrap();
    let decoded = Macaroon::from_base64(&challenge.macaroon).unwrap();
    let session_id = SessionBudgetManager::extract_session_caveats(&decoded)
        .0
        .unwrap();

    // 2. Settle invoice
    let preimage = lightning.pay_invoice(&challenge.invoice).await.unwrap();
    let auth_val = format!("L402 {}:{}", challenge.macaroon, preimage);

    // 3. First authenticated request is STREAMING!
    let chat_body = json!({
        "model": "llama3.2",
        "stream": true,
        "messages": [{"role": "user", "content": "Stream please"}]
    });

    let resp_stream = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("Content-Type", "application/json")
                .header("Authorization", &auth_val)
                .body(Body::from(serde_json::to_vec(&chat_body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp_stream.status(), StatusCode::OK);
    assert_eq!(
        resp_stream
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "text/event-stream"
    );

    // Read full stream
    let body_bytes = axum::body::to_bytes(resp_stream.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
    assert!(body_str.contains("Hello world"));
    assert!(body_str.contains("[DONE]"));

    // 4. Verify session transitioned to active and exactly 1 funding entry exists
    let rec_active = db
        .session_store()
        .get_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        rec_active.status,
        satsgrid::node::storage::SessionStatus::Active
    );

    let entries = db
        .session_store()
        .get_accounting_entries(&session_id)
        .await
        .unwrap();
    let funding_entries: Vec<_> = entries
        .iter()
        .filter(|e| e.1 == "session_funding")
        .collect();
    assert_eq!(funding_entries.len(), 1);

    // Verify accounting invariant: remaining + reserved + spent == total
    assert_eq!(
        rec_active.remaining_sats.0 + rec_active.reserved_sats.0 + rec_active.spent_sats.0,
        rec_active.total_budget_sats.0
    );
}
