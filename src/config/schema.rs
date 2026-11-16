use crate::common::types::Satoshis;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LightningBackendType {
    #[default]
    Mock,
    Lnd,
    Nwc,
}

fn default_max_request_body_bytes() -> usize {
    2 * 1024 * 1024 // 2 MB
}

fn default_max_completion_tokens_limit() -> usize {
    128_000 // 128k tokens maximum boundary
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub admin_token: Option<String>,
    #[serde(default = "default_max_request_body_bytes")]
    pub max_request_body_bytes: usize,
    #[serde(default = "default_max_completion_tokens_limit")]
    pub max_completion_tokens_limit: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8080,
            admin_token: None,
            max_request_body_bytes: default_max_request_body_bytes(),
            max_completion_tokens_limit: default_max_completion_tokens_limit(),
        }
    }
}

fn default_connect_timeout_secs() -> u64 {
    10
}

fn default_response_header_timeout_secs() -> u64 {
    60
}

fn default_stream_idle_timeout_secs() -> u64 {
    30
}

fn default_total_request_timeout_secs() -> u64 {
    600
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamConfig {
    pub url: String,
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
    #[serde(default = "default_response_header_timeout_secs")]
    pub response_header_timeout_secs: u64,
    #[serde(default = "default_stream_idle_timeout_secs")]
    pub stream_idle_timeout_secs: u64,
    #[serde(default = "default_total_request_timeout_secs")]
    pub total_request_timeout_secs: u64,
}

impl UpstreamConfig {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            ..Default::default()
        }
    }
}

impl Default for UpstreamConfig {
    fn default() -> Self {
        Self {
            url: "http://127.0.0.1:11434".to_string(),
            connect_timeout_secs: default_connect_timeout_secs(),
            response_header_timeout_secs: default_response_header_timeout_secs(),
            stream_idle_timeout_secs: default_stream_idle_timeout_secs(),
            total_request_timeout_secs: default_total_request_timeout_secs(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PricingConfig {
    #[serde(default = "default_price_sats", alias = "sats_per_request")]
    pub default_price_sats: Satoshis,
    #[serde(default)]
    pub sats_per_prompt_token: u64,
    #[serde(default)]
    pub sats_per_completion_token: u64,
}

fn default_price_sats() -> Satoshis {
    Satoshis(10)
}

impl PricingConfig {
    pub fn new(default_price_sats: Satoshis) -> Self {
        Self {
            default_price_sats,
            sats_per_prompt_token: 0,
            sats_per_completion_token: 0,
        }
    }
}

impl Default for PricingConfig {
    fn default() -> Self {
        Self {
            default_price_sats: Satoshis(10),
            sats_per_prompt_token: 0,
            sats_per_completion_token: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BitcoinNetwork {
    Mainnet,
    #[default]
    Testnet,
    Regtest,
    Signet,
}

fn default_pay_per_request_expiry_secs() -> u64 {
    3600 // 1 hour
}

fn default_session_funding_expiry_secs() -> u64 {
    86400 * 30 // 30 days
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LightningConfig {
    pub backend: LightningBackendType,
    #[serde(default)]
    pub network: BitcoinNetwork,
    #[serde(alias = "lnd_rest_host")]
    pub lnd_rpc_host: Option<String>,
    pub lnd_macaroon_path: Option<String>,
    pub lnd_tls_cert_path: Option<String>,
    pub nwc_uri: Option<String>,
    #[serde(default = "default_pay_per_request_expiry_secs")]
    pub pay_per_request_expiry_secs: u64,
    #[serde(default = "default_session_funding_expiry_secs")]
    pub session_funding_expiry_secs: u64,
}

impl Default for LightningConfig {
    fn default() -> Self {
        Self {
            backend: LightningBackendType::Mock,
            network: BitcoinNetwork::Testnet,
            lnd_rpc_host: None,
            lnd_macaroon_path: None,
            lnd_tls_cert_path: None,
            nwc_uri: None,
            pay_per_request_expiry_secs: default_pay_per_request_expiry_secs(),
            session_funding_expiry_secs: default_session_funding_expiry_secs(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_discovery_relays")]
    pub relays: Vec<String>,
    #[serde(default)]
    pub node_name: Option<String>,
    #[serde(default)]
    pub public_url: Option<String>,
    #[serde(default)]
    pub nsec: Option<String>,
}

fn default_discovery_relays() -> Vec<String> {
    vec![
        "wss://relay.damus.io".to_string(),
        "wss://nos.lol".to_string(),
    ]
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            relays: default_discovery_relays(),
            node_name: None,
            public_url: None,
            nsec: None,
        }
    }
}

fn default_max_concurrent_requests() -> usize {
    32
}

fn default_max_concurrent_per_session() -> usize {
    4
}

fn default_admission_timeout_secs() -> u64 {
    0
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConcurrencyConfig {
    #[serde(default = "default_max_concurrent_requests")]
    pub max_concurrent_requests: usize,
    #[serde(default = "default_max_concurrent_per_session")]
    pub max_concurrent_per_session: usize,
    #[serde(default = "default_admission_timeout_secs")]
    pub admission_timeout_secs: u64,
}

impl Default for ConcurrencyConfig {
    fn default() -> Self {
        Self {
            max_concurrent_requests: default_max_concurrent_requests(),
            max_concurrent_per_session: default_max_concurrent_per_session(),
            admission_timeout_secs: default_admission_timeout_secs(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NodeConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub upstream: UpstreamConfig,
    #[serde(default)]
    pub pricing: PricingConfig,
    #[serde(default)]
    pub lightning: LightningConfig,
    #[serde(default)]
    pub discovery: DiscoveryConfig,
    #[serde(default)]
    pub concurrency: ConcurrencyConfig,
    #[serde(default = "default_data_dir")]
    pub data_dir: String,
}

fn default_data_dir() -> String {
    ".satsgrid".to_string()
}

impl NodeConfig {
    pub fn from_file(path: impl AsRef<std::path::Path>) -> crate::common::error::Result<Self> {
        let contents = std::fs::read_to_string(&path).map_err(|e| {
            crate::common::error::Error::Config(format!(
                "Failed to read config file {}: {}",
                path.as_ref().display(),
                e
            ))
        })?;
        let config: Self = toml::from_str(&contents).map_err(|e| {
            crate::common::error::Error::Config(format!("Failed to parse TOML config: {}", e))
        })?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> crate::common::error::Result<()> {
        if self.upstream.connect_timeout_secs == 0 {
            return Err(crate::common::error::Error::Config(
                "upstream.connect_timeout_secs must be greater than 0".to_string(),
            ));
        }
        if self.upstream.response_header_timeout_secs < self.upstream.connect_timeout_secs {
            return Err(crate::common::error::Error::Config(
                "upstream.response_header_timeout_secs cannot be less than connect_timeout_secs"
                    .to_string(),
            ));
        }
        if self.upstream.stream_idle_timeout_secs == 0 {
            return Err(crate::common::error::Error::Config(
                "upstream.stream_idle_timeout_secs must be greater than 0".to_string(),
            ));
        }
        if self.upstream.total_request_timeout_secs < self.upstream.response_header_timeout_secs {
            return Err(crate::common::error::Error::Config(
                "upstream.total_request_timeout_secs cannot be less than response_header_timeout_secs"
                    .to_string(),
            ));
        }
        if self.concurrency.max_concurrent_requests == 0 {
            return Err(crate::common::error::Error::Config(
                "concurrency.max_concurrent_requests must be greater than 0".to_string(),
            ));
        }
        if self.concurrency.max_concurrent_per_session == 0 {
            return Err(crate::common::error::Error::Config(
                "concurrency.max_concurrent_per_session must be greater than 0".to_string(),
            ));
        }
        if self.concurrency.max_concurrent_per_session > self.concurrency.max_concurrent_requests {
            return Err(crate::common::error::Error::Config(
                "concurrency.max_concurrent_per_session cannot exceed max_concurrent_requests"
                    .to_string(),
            ));
        }
        if self.lightning.pay_per_request_expiry_secs < 60
            || self.lightning.pay_per_request_expiry_secs > 86400 * 7
        {
            return Err(crate::common::error::Error::Config(
                "lightning.pay_per_request_expiry_secs must be between 60 and 604800 (7 days)"
                    .to_string(),
            ));
        }
        if self.lightning.session_funding_expiry_secs < 3600
            || self.lightning.session_funding_expiry_secs > 86400 * 90
        {
            return Err(crate::common::error::Error::Config(
                "lightning.session_funding_expiry_secs must be between 3600 and 7776000 (90 days)"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientEndpointConfig {
    pub node_url: String,
}

impl Default for ClientEndpointConfig {
    fn default() -> Self {
        Self {
            node_url: "http://127.0.0.1:8080".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetConfig {
    pub max_budget_sats: Option<Satoshis>,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            max_budget_sats: Some(Satoshis(1000)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientPaymentConfig {
    pub payment_backend: LightningBackendType,
}

impl Default for ClientPaymentConfig {
    fn default() -> Self {
        Self {
            payment_backend: LightningBackendType::Mock,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ClientConfig {
    #[serde(default)]
    pub client: ClientEndpointConfig,
    #[serde(default)]
    pub budget: BudgetConfig,
    #[serde(default)]
    pub lightning: ClientPaymentConfig,
}
