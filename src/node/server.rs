use crate::common::error::Result;
use crate::config::schema::LightningBackendType;
use crate::config::schema::NodeConfig;
use crate::node::api::routes::create_routes;
use crate::node::api::AppState;
use crate::node::gate::{MacaroonService, SessionBudgetManager};
use crate::node::lightning::mock::MockLightningBackend;
use crate::node::lightning::nwc::NwcLightningBackend;
use crate::node::lightning::LightningBackend;
use crate::node::proxy::openai::OpenAiProxy;
use crate::node::storage::Database;
use std::sync::Arc;
use tower_http::trace::TraceLayer;

pub struct SatsGridServer {
    pub config: NodeConfig,
}

impl SatsGridServer {
    pub fn new(config: NodeConfig) -> Self {
        Self { config }
    }

    pub async fn run(&self) -> Result<()> {
        let shutdown_signal = async {
            let ctrl_c = async {
                tokio::signal::ctrl_c()
                    .await
                    .expect("failed to install Ctrl+C handler");
            };

            #[cfg(unix)]
            let terminate = async {
                match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                    Ok(mut sig) => {
                        sig.recv().await;
                    }
                    Err(e) => {
                        tracing::warn!("Failed to install SIGTERM signal handler: {}", e);
                        std::future::pending::<()>().await;
                    }
                }
            };

            #[cfg(not(unix))]
            let terminate = std::future::pending::<()>();

            tokio::select! {
                _ = ctrl_c => {
                    tracing::info!("Received Ctrl+C (SIGINT), initiating graceful shutdown...");
                },
                _ = terminate => {
                    tracing::info!("Received SIGTERM, initiating graceful shutdown...");
                },
            }
        };

        self.run_until_shutdown(shutdown_signal).await
    }

    pub async fn run_until_shutdown<F>(&self, shutdown: F) -> Result<()>
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let addr = format!("{}:{}", self.config.server.host, self.config.server.port);
        tracing::info!("Starting SatsGrid Node on {}", addr);

        // Initialize persistent SQLite database in data_dir
        let db = Database::init(&self.config.data_dir).await?;

        // Run startup crash recovery for in-flight requests and session reservations
        let req_store = db.request_store();
        match req_store.recover_in_flight_requests().await {
            Ok(undispatched) => {
                let undispatched_ids: Vec<String> =
                    undispatched.into_iter().map(|(res_id, _)| res_id).collect();
                if let Err(e) = db
                    .session_store()
                    .recover_in_flight_reservations(&undispatched_ids)
                    .await
                {
                    tracing::warn!("Session recovery encountered error: {}", e);
                }
            }
            Err(e) => {
                tracing::warn!("Request recovery encountered error: {}", e);
            }
        }

        // Initialize persistent session budget manager
        let budget_manager = Arc::new(SessionBudgetManager::with_store(db.session_store()));

        let macaroon_key = Self::load_or_generate_macaroon_key(&self.config.data_dir)?;
        let admin_token = Self::load_or_generate_admin_token(&self.config)?;

        let proxy = OpenAiProxy::new(self.config.upstream.url.clone());

        let lightning_backend: Arc<dyn LightningBackend> = match self.config.lightning.backend {
            LightningBackendType::Mock => {
                tracing::info!("Initializing Mock Lightning backend");
                Arc::new(MockLightningBackend::new())
            }
            LightningBackendType::Nwc => {
                tracing::info!("Connecting to NWC backend");
                let uri = std::env::var("SATSGRID_NWC_URI")
                    .or_else(|_| {
                        self.config
                            .lightning
                            .nwc_uri
                            .clone()
                            .ok_or("Missing SATSGRID_NWC_URI".to_string())
                    })
                    .map_err(|e| {
                        crate::common::error::Error::Config(format!(
                            "NWC configuration error: {}",
                            e
                        ))
                    })?;
                Arc::new(NwcLightningBackend::new(uri)?)
            }
            LightningBackendType::Lnd => {
                tracing::info!(
                    "Connecting to LND REST backend on {} ({:?})",
                    self.config.lightning.lnd_rpc_host.as_deref().unwrap_or(""),
                    self.config.lightning.network
                );
                let backend =
                    crate::node::lightning::LndBackend::from_config(&self.config.lightning)?;
                Arc::new(backend)
            }
        };

        let state = AppState {
            config: Arc::new(self.config.clone()),
            live_pricing: Arc::new(tokio::sync::RwLock::new(self.config.pricing.clone())),
            lightning: lightning_backend,
            budget_manager,
            macaroon_service: Arc::new(MacaroonService::new(macaroon_key, "satsgrid-node")),
            proxy,
            stats: Arc::new(crate::node::api::NodeStats::default()),
            admin_token: Arc::new(admin_token),
            db: Some(Arc::new(db)),
        };

        let app = create_routes(state).layer(
            TraceLayer::new_for_http()
                .make_span_with(tower_http::trace::DefaultMakeSpan::new().include_headers(false)),
        );

        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|e| crate::common::error::Error::Internal(e.to_string()))?;

        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(|e| crate::common::error::Error::Internal(e.to_string()))?;

        Ok(())
    }

    fn load_or_generate_macaroon_key(data_dir: &str) -> Result<Vec<u8>> {
        use rand::RngCore;
        use std::fs;
        use std::path::Path;

        let data_path = Path::new(data_dir);
        if !data_path.exists() {
            fs::create_dir_all(data_path).map_err(|e| {
                crate::common::error::Error::Internal(format!(
                    "Failed to create data directory: {}",
                    e
                ))
            })?;
        }

        let key_path = data_path.join(".satsgrid_macaroon_key");

        if key_path.exists() {
            let key = fs::read(&key_path).map_err(|e| {
                crate::common::error::Error::Internal(format!("Failed to read macaroon key: {}", e))
            })?;
            if key.len() == 32 {
                return Ok(key);
            }
        }

        tracing::info!(
            "Generating new macaroon root key and saving to {}",
            key_path.display()
        );
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);

        fs::write(&key_path, key).map_err(|e| {
            crate::common::error::Error::Internal(format!(
                "Failed to persist Macaroon root key: {}",
                e
            ))
        })?;

        Ok(key.to_vec())
    }

    fn load_or_generate_admin_token(config: &NodeConfig) -> Result<String> {
        use rand::RngCore;
        use std::fs;
        use std::path::Path;

        // 1. Environment variable override
        if let Ok(env_token) = std::env::var("SATSGRID_ADMIN_TOKEN") {
            let trimmed = env_token.trim().to_string();
            if !trimmed.is_empty() {
                return Ok(trimmed);
            }
        }

        // 2. Explicit config file option
        if let Some(token) = &config.server.admin_token {
            let trimmed = token.trim().to_string();
            if !trimmed.is_empty() {
                return Ok(trimmed);
            }
        }

        // 3. Persist or read from data_dir/.satsgrid_admin_token
        let data_path = Path::new(&config.data_dir);
        if !data_path.exists() {
            fs::create_dir_all(data_path).map_err(|e| {
                crate::common::error::Error::Internal(format!(
                    "Failed to create data directory: {}",
                    e
                ))
            })?;
        }

        let token_path = data_path.join(".satsgrid_admin_token");
        if token_path.exists() {
            let token = fs::read_to_string(&token_path).map_err(|e| {
                crate::common::error::Error::Internal(format!("Failed to read admin token: {}", e))
            })?;
            let trimmed = token.trim().to_string();
            if !trimmed.is_empty() {
                return Ok(trimmed);
            }
        }

        let mut token_bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut token_bytes);
        let new_token = hex::encode(token_bytes);

        fs::write(&token_path, &new_token).map_err(|e| {
            crate::common::error::Error::Internal(format!("Failed to persist admin token: {}", e))
        })?;

        tracing::info!(
            "Generated new node admin token and saved to {}",
            token_path.display()
        );

        Ok(new_token)
    }
}
