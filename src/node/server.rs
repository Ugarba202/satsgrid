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
        self.config.validate()?;
        let host = &self.config.server.host;
        let addr = if host.contains(':') {
            format!("[{}]:{}", host, self.config.server.port)
        } else {
            format!("{}:{}", host, self.config.server.port)
        };
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

        let proxy = OpenAiProxy::new_with_timeouts(
            self.config.upstream.url.clone(),
            std::time::Duration::from_secs(self.config.upstream.connect_timeout_secs),
            std::time::Duration::from_secs(self.config.upstream.response_header_timeout_secs),
        );

        let concurrency_manager = Arc::new(crate::node::concurrency::ConcurrencyManager::new(
            self.config.concurrency.max_concurrent_requests,
            self.config.concurrency.max_concurrent_per_session,
            self.config.concurrency.admission_timeout_secs,
        ));

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

        let db_arc = Arc::new(db);

        // Synchronize models from upstream into durable ModelStore on startup
        {
            let model_store = db_arc.model_store();
            match proxy.list_models().await {
                Ok(val) => {
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
                        if let Err(e) = model_store
                            .upsert_discovered(&discovered_models, true)
                            .await
                        {
                            tracing::warn!(
                                "Failed to upsert discovered models into ModelStore: {}",
                                e
                            );
                        } else {
                            tracing::info!(
                                "Synchronized {} models into durable ModelStore",
                                discovered_models.len()
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Upstream models unavailable on startup (existing catalog preserved): {}",
                        e
                    );
                }
            }
        }

        let recon_worker = Arc::new(crate::node::worker::ReconciliationWorker::new(
            db_arc.clone(),
            lightning_backend.clone(),
            crate::node::worker::WorkerConfig::default(),
        ));

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let worker_handle = recon_worker.clone().spawn(shutdown_rx);
        AppState::set_reconciliation_worker(recon_worker);

        let receipt_signer = Arc::new(crate::node::receipt::ReceiptSigner::load_or_generate(
            &self.config.data_dir,
            "satsgrid-node",
        )?);
        AppState::set_receipt_signer(receipt_signer);

        let state = AppState {
            config: Arc::new(self.config.clone()),
            live_pricing: Arc::new(tokio::sync::RwLock::new(self.config.pricing.clone())),
            lightning: lightning_backend,
            budget_manager,
            macaroon_service: Arc::new(MacaroonService::new(macaroon_key, "satsgrid-node")),
            proxy,
            stats: Arc::new(crate::node::api::NodeStats::default()),
            admin_token: Arc::new(admin_token),
            db: Some(db_arc),
            concurrency_manager,
        };

        let app = create_routes(state).layer(
            TraceLayer::new_for_http()
                .make_span_with(tower_http::trace::DefaultMakeSpan::new().include_headers(false)),
        );

        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|e| crate::common::error::Error::Internal(e.to_string()))?;

        let server_shutdown = async move {
            shutdown.await;
            let _ = shutdown_tx.send(true);
            let _ = worker_handle.await;
        };

        axum::serve(listener, app)
            .with_graceful_shutdown(server_shutdown)
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

    pub fn load_or_generate_nostr_identity(
        data_dir: &str,
        configured_nsec: Option<&str>,
    ) -> Result<nostr_sdk::prelude::Keys> {
        use nostr_sdk::prelude::Keys;
        use std::fs;
        use std::path::Path;

        // 1. Explicit configured nsec
        if let Some(nsec) = configured_nsec {
            let trimmed = nsec.trim();
            if !trimmed.is_empty() {
                return Keys::parse(trimmed).map_err(|e| {
                    crate::common::error::Error::Config(format!("Invalid configured nsec: {}", e))
                });
            }
        }

        // 2. Read from persistent file data_dir/.satsgrid_nostr_key
        let data_path = Path::new(data_dir);
        if !data_path.exists() {
            fs::create_dir_all(data_path).map_err(|e| {
                crate::common::error::Error::Internal(format!(
                    "Failed to create data directory: {}",
                    e
                ))
            })?;
        }

        let key_path = data_path.join(".satsgrid_nostr_key");
        if key_path.exists() {
            let secret_str = fs::read_to_string(&key_path).map_err(|e| {
                crate::common::error::Error::Internal(format!(
                    "Failed to read Nostr identity key: {}",
                    e
                ))
            })?;
            let trimmed = secret_str.trim();
            if !trimmed.is_empty() {
                return Keys::parse(trimmed).map_err(|e| {
                    crate::common::error::Error::Config(format!(
                        "Invalid stored Nostr identity key: {}",
                        e
                    ))
                });
            }
        }

        // 3. Generate new persistent Nostr identity
        let new_keys = Keys::generate();
        let secret_hex = new_keys.secret_key().to_secret_hex();
        fs::write(&key_path, &secret_hex).map_err(|e| {
            crate::common::error::Error::Internal(format!(
                "Failed to persist Nostr identity key: {}",
                e
            ))
        })?;

        tracing::info!(
            "Generated new persistent Nostr identity (pubkey: {}) and saved to {}",
            new_keys.public_key().to_hex(),
            key_path.display()
        );

        Ok(new_keys)
    }
}
