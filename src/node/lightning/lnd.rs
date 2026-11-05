use crate::common::error::{Error, Result};
use crate::common::types::{PaymentHash, Satoshis};
use crate::config::schema::LightningConfig;
use crate::node::lightning::backend::LightningBackend;
use crate::node::lightning::invoice::Invoice;
use async_trait::async_trait;
use base64::Engine;
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::Client;
use serde_json::{json, Value};
use std::fs;

#[derive(Clone)]
pub struct LndBackend {
    client: Client,
    rest_url: String,
    macaroon_hex: String,
}

impl LndBackend {
    pub fn new(
        rest_url: impl Into<String>,
        macaroon_hex: impl Into<String>,
        client: Client,
    ) -> Self {
        let mut url = rest_url.into();
        if !url.starts_with("http://") && !url.starts_with("https://") {
            url = format!("https://{}", url);
        }
        let url = url.trim_end_matches('/').to_string();

        Self {
            client,
            rest_url: url,
            macaroon_hex: macaroon_hex.into(),
        }
    }

    pub fn from_config(config: &LightningConfig) -> Result<Self> {
        let rest_host = config.lnd_rpc_host.as_deref().ok_or_else(|| {
            Error::Config("Missing lnd_rpc_host / lnd_rest_host in configuration".to_string())
        })?;

        let macaroon_path = config.lnd_macaroon_path.as_deref().ok_or_else(|| {
            Error::Config("Missing lnd_macaroon_path in configuration".to_string())
        })?;

        let macaroon_bytes = Self::read_file_expanded(macaroon_path).map_err(|e| {
            Error::Config(format!(
                "Failed to read LND macaroon from {}: {}",
                macaroon_path, e
            ))
        })?;
        let macaroon_hex = hex::encode(macaroon_bytes);

        let mut client_builder = Client::builder();

        if let Some(cert_path) = config.lnd_tls_cert_path.as_deref() {
            let cert_bytes = Self::read_file_expanded(cert_path).map_err(|e| {
                Error::Config(format!(
                    "Failed to read LND tls.cert from {}: {}",
                    cert_path, e
                ))
            })?;
            if let Ok(cert) = reqwest::Certificate::from_pem(&cert_bytes) {
                client_builder = client_builder.add_root_certificate(cert);
            }
        }

        // Allow self-signed certificates and loopback hostname variations for local and Polar nodes
        client_builder = client_builder
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true);

        let client = client_builder
            .build()
            .map_err(|e| Error::Lightning(format!("Failed to build HTTP client for LND: {}", e)))?;

        Ok(Self::new(rest_host, macaroon_hex, client))
    }

    fn read_file_expanded(path_str: &str) -> std::io::Result<Vec<u8>> {
        let path = if path_str.starts_with("~/") || path_str.starts_with("~\\") {
            if let Some(home) = dirs_home() {
                home.join(&path_str[2..])
            } else {
                std::path::PathBuf::from(path_str)
            }
        } else {
            #[cfg(windows)]
            if path_str.starts_with("/mnt/") && path_str.len() > 6 {
                let drive = path_str.chars().nth(5).unwrap();
                let rest = &path_str[6..];
                std::path::PathBuf::from(format!("{}:{}", drive, rest))
            } else {
                std::path::PathBuf::from(path_str)
            }

            #[cfg(not(windows))]
            if path_str.len() > 2 && path_str.chars().nth(1) == Some(':') {
                let drive = path_str.chars().next().unwrap().to_ascii_lowercase();
                let rest = &path_str[2..].replace('\\', "/");
                std::path::PathBuf::from(format!("/mnt/{}{}", drive, rest))
            } else {
                std::path::PathBuf::from(path_str)
            }
        };
        fs::read(path)
    }

    fn auth_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Ok(val) = HeaderValue::from_str(&self.macaroon_hex) {
            headers.insert("Grpc-Metadata-macaroon", val);
        }
        headers
    }
}

fn dirs_home() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(std::path::PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(std::path::PathBuf::from)
    }
}

#[async_trait]
impl LightningBackend for LndBackend {
    async fn create_invoice(&self, amount_sats: Satoshis, memo: &str) -> Result<Invoice> {
        let url = format!("{}/v1/invoices", self.rest_url);
        let payload = json!({
            "value": amount_sats.0,
            "memo": memo
        });

        let resp = self
            .client
            .post(&url)
            .headers(self.auth_headers())
            .json(&payload)
            .send()
            .await
            .map_err(|e| Error::Lightning(format!("LND connection error: {}", e)))?;

        if !resp.status().is_success() {
            let err_text = resp.text().await.unwrap_or_default();
            return Err(Error::Lightning(format!(
                "LND error creating invoice: {}",
                err_text
            )));
        }

        let body: Value = resp.json().await.map_err(|e| {
            Error::Lightning(format!("Failed to parse LND invoice response: {}", e))
        })?;

        let payment_request = body["payment_request"]
            .as_str()
            .ok_or_else(|| Error::Lightning("LND response missing payment_request".to_string()))?
            .to_string();

        let r_hash_b64 = body["r_hash"]
            .as_str()
            .ok_or_else(|| Error::Lightning("LND response missing r_hash".to_string()))?;

        let hash_bytes = base64::engine::general_purpose::STANDARD
            .decode(r_hash_b64)
            .map_err(|e| Error::Lightning(format!("Invalid r_hash base64 from LND: {}", e)))?;
        let payment_hash_hex = hex::encode(hash_bytes);

        Ok(Invoice {
            bolt11: payment_request,
            payment_hash: PaymentHash(payment_hash_hex),
            amount: amount_sats,
        })
    }

    async fn is_invoice_settled(&self, payment_hash: &PaymentHash) -> Result<bool> {
        let url = format!("{}/v1/invoice/{}", self.rest_url, payment_hash.0);

        let resp = self
            .client
            .get(&url)
            .headers(self.auth_headers())
            .send()
            .await
            .map_err(|e| Error::Lightning(format!("LND connection error: {}", e)))?;

        if resp.status().as_u16() == 404 {
            return Ok(false);
        }

        if !resp.status().is_success() {
            let err_text = resp.text().await.unwrap_or_default();
            return Err(Error::Lightning(format!(
                "LND error checking invoice: {}",
                err_text
            )));
        }

        let body: Value = resp
            .json()
            .await
            .map_err(|e| Error::Lightning(format!("Failed to parse LND invoice status: {}", e)))?;

        let is_settled = body["settled"].as_bool().unwrap_or(false)
            || body["state"]
                .as_str()
                .map(|s| s == "SETTLED")
                .unwrap_or(false);

        Ok(is_settled)
    }

    async fn pay_invoice(&self, invoice: &str) -> Result<String> {
        self.pay_invoice(invoice).await
    }
}

impl LndBackend {
    pub async fn pay_invoice(&self, invoice: &str) -> Result<String> {
        let url = format!("{}/v1/channels/transactions", self.rest_url);
        let payload = json!({
            "payment_request": invoice
        });

        let resp = self
            .client
            .post(&url)
            .headers(self.auth_headers())
            .json(&payload)
            .send()
            .await
            .map_err(|e| Error::Lightning(format!("LND connection error: {}", e)))?;

        if !resp.status().is_success() {
            let err_text = resp.text().await.unwrap_or_default();
            return Err(Error::Lightning(format!("LND payment error: {}", err_text)));
        }

        let body: Value = resp.json().await.map_err(|e| {
            Error::Lightning(format!("Failed to parse LND payment response: {}", e))
        })?;

        if let Some(err) = body.get("payment_error").and_then(|e| e.as_str()) {
            if !err.is_empty() {
                return Err(Error::Lightning(format!("LND payment failed: {}", err)));
            }
        }

        let preimage_str = body["payment_preimage"]
            .as_str()
            .ok_or_else(|| Error::Lightning("LND response missing payment_preimage".to_string()))?;

        // If returned in base64, decode and hex encode
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(preimage_str) {
            if bytes.len() == 32 {
                return Ok(hex::encode(bytes));
            }
        }

        Ok(preimage_str.to_string())
    }
}

#[async_trait]
impl crate::client::pay::LightningPaymentProvider for LndBackend {
    async fn pay_invoice(
        &self,
        invoice: &str,
    ) -> std::result::Result<String, crate::client::error::ClientError> {
        LndBackend::pay_invoice(self, invoice)
            .await
            .map_err(|e| crate::client::error::ClientError::Payment(e.to_string()))
    }
}
