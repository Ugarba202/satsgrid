use super::handlers;
use super::AppState;
use axum::{
    routing::{get, post},
    Router,
};

pub fn create_routes(state: AppState) -> Router {
    Router::new()
        .route("/health", get(handlers::health_check))
        .route("/v1/models", get(handlers::models))
        .route("/v1/session/new", post(handlers::new_session))
        .route(
            "/v1/session/close",
            post(super::session_closure::close_session),
        )
        .route("/v1/chat/completions", post(handlers::chat_completions))
        .route("/v1/node/stats", get(handlers::node_stats))
        .route(
            "/v1/node/config",
            get(handlers::node_config).post(handlers::update_node_config),
        )
        .route("/internal/mock/pay", post(handlers::mock_pay))
        .with_state(state)
}
