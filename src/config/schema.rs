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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub admin_token: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8080,
            admin_token: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamConfig {
    pub url: String,
}

impl Default for UpstreamConfig {
    fn default() -> Self {
        Self {
            url: "http://127.0.0.1:11434".to_string(),
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
        Ok(config)
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
