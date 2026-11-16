pub mod handlers;
pub mod routes;
pub mod validation;

use crate::config::schema::{NodeConfig, PricingConfig};
use crate::node::concurrency::ConcurrencyManager;
use crate::node::gate::{MacaroonService, SessionBudgetManager};
use crate::node::lightning::backend::LightningBackend;
use crate::node::pricing::PricingCalculator;
use crate::node::proxy::openai::OpenAiProxy;
use crate::node::storage::Database;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Default)]
pub struct NodeStats {
    pub total_requests: AtomicU64,
    pub total_sats_earned: AtomicU64,
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<NodeConfig>,
    pub live_pricing: Arc<RwLock<PricingConfig>>,
    pub lightning: Arc<dyn LightningBackend>,
    pub budget_manager: Arc<SessionBudgetManager>,
    pub macaroon_service: Arc<MacaroonService>,
    pub proxy: OpenAiProxy,
    pub stats: Arc<NodeStats>,
    pub admin_token: Arc<String>,
    pub db: Option<Arc<Database>>,
    pub concurrency_manager: Arc<ConcurrencyManager>,
}

impl AppState {
    pub async fn pricing_calculator(&self) -> PricingCalculator {
        let pricing = self.live_pricing.read().await.clone();
        PricingCalculator::new(pricing)
    }

    pub fn verify_admin(
        &self,
        headers: &axum::http::HeaderMap,
    ) -> crate::common::error::Result<()> {
        let auth_val = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| {
                crate::common::error::Error::Unauthorized(
                    "Admin authorization required: missing Authorization header".to_string(),
                )
            })?;

        let token = auth_val.strip_prefix("Bearer ").ok_or_else(|| {
            crate::common::error::Error::Unauthorized(
                "Invalid authorization scheme: Bearer token required".to_string(),
            )
        })?;

        if token == self.admin_token.as_str() {
            Ok(())
        } else {
            Err(crate::common::error::Error::Unauthorized(
                "Invalid admin token".to_string(),
            ))
        }
    }
}
