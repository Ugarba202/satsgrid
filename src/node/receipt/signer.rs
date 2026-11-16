use crate::common::error::{Error, Result};
use ring::rand::SystemRandom;
use ring::signature::{self, KeyPair};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

pub const RECEIPT_DOMAIN_PREFIX: &str = "satsgrid:receipt:v1:";
pub const ED25519_ALGORITHM_NAME: &str = "ed25519";

/// Recursively sort JSON keys to guarantee deterministic serialization (RFC 8785 subset).
pub fn sort_json_keys(val: &Value) -> Value {
    match val {
        Value::Object(map) => {
            let mut sorted = BTreeMap::new();
            for (k, v) in map {
                sorted.insert(k.clone(), sort_json_keys(v));
            }
            let mut new_map = serde_json::Map::new();
            for (k, v) in sorted {
                new_map.insert(k, v);
            }
            Value::Object(new_map)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(sort_json_keys).collect()),
        other => other.clone(),
    }
}

/// Public cryptographic receipt payload describing inference and payment facts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReceiptPayload {
    pub receipt_id: String,
    pub schema_version: u32,
    pub key_id: String,
    pub provider_id: String,
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub model: String,
    pub inference_status: String,
    pub payment_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_hash: Option<String>,
    pub approved_max_sats: u64,
    pub actual_charge_sats: u64,
    pub currency: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconciliation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes_receipt_id: Option<String>,
    pub created_at: i64,
}

impl ReceiptPayload {
    /// Serialize payload to deterministic canonical JSON with sorted keys and no whitespace.
    pub fn to_canonical_json(&self) -> Result<String> {
        let val = serde_json::to_value(self)
            .map_err(|e| Error::Internal(format!("Failed to serialize receipt: {}", e)))?;
        let canonical_val = sort_json_keys(&val);
        serde_json::to_string(&canonical_val)
            .map_err(|e| Error::Internal(format!("Failed to format canonical json: {}", e)))
    }

    /// Produce the exact bytes that are signed (domain prefix + canonical JSON).
    pub fn to_signed_bytes(&self) -> Result<Vec<u8>> {
        let canonical_json = self.to_canonical_json()?;
        let mut bytes = Vec::with_capacity(RECEIPT_DOMAIN_PREFIX.len() + canonical_json.len());
        bytes.extend_from_slice(RECEIPT_DOMAIN_PREFIX.as_bytes());
        bytes.extend_from_slice(canonical_json.as_bytes());
        Ok(bytes)
    }
}

/// Cryptographic container representing a signed receipt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignedReceipt {
    pub payload: ReceiptPayload,
    pub signature_hex: String,
    pub public_key_hex: String,
    pub algorithm: String,
    pub key_id: String,
}

/// Ed25519 signer managing receipt key custody and signature generation.
pub struct ReceiptSigner {
    key_pair: signature::Ed25519KeyPair,
    key_id: String,
    public_key_hex: String,
    provider_id: String,
}

impl ReceiptSigner {
    /// Load or securely generate the persistent Ed25519 receipt signing key.
    pub fn load_or_generate(
        data_dir: impl AsRef<Path>,
        provider_id: impl Into<String>,
    ) -> Result<Self> {
        let path = data_dir.as_ref().join(".satsgrid_receipt_key.pk8");
        let pkcs8_bytes = if path.exists() {
            fs::read(&path).map_err(|e| {
                Error::Config(format!(
                    "Failed to read receipt private key from '{}': {}",
                    path.display(),
                    e
                ))
            })?
        } else {
            let rng = SystemRandom::new();
            let new_bytes = signature::Ed25519KeyPair::generate_pkcs8(&rng)
                .map_err(|e| Error::Internal(format!("Failed to generate Ed25519 key: {:?}", e)))?;

            // Set 0600 file permissions on unix
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                let mut options = fs::OpenOptions::new();
                options.write(true).create_new(true).mode(0o600);
                use std::io::Write;
                let mut file = options.open(&path).map_err(|e| {
                    Error::Config(format!(
                        "Failed to create receipt key file '{}': {}",
                        path.display(),
                        e
                    ))
                })?;
                file.write_all(new_bytes.as_ref()).map_err(|e| {
                    Error::Config(format!(
                        "Failed to write receipt key file '{}': {}",
                        path.display(),
                        e
                    ))
                })?;
            }

            #[cfg(not(unix))]
            {
                fs::write(&path, new_bytes.as_ref()).map_err(|e| {
                    Error::Config(format!("Failed to write receipt key file: {}", e))
                })?;
            }

            new_bytes.as_ref().to_vec()
        };

        let key_pair = signature::Ed25519KeyPair::from_pkcs8(&pkcs8_bytes)
            .map_err(|e| Error::Config(format!("Invalid Ed25519 PKCS#8 key: {:?}", e)))?;

        let pubkey_bytes = key_pair.public_key().as_ref();
        let public_key_hex = hex::encode(pubkey_bytes);
        let key_id = format!("ed25519:{}", &public_key_hex[..16]);

        Ok(Self {
            key_pair,
            key_id,
            public_key_hex,
            provider_id: provider_id.into(),
        })
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn public_key_hex(&self) -> &str {
        &self.public_key_hex
    }

    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    /// Sign a receipt payload and return a verifiable `SignedReceipt`.
    pub fn sign_receipt(&self, mut payload: ReceiptPayload) -> Result<SignedReceipt> {
        payload.key_id = self.key_id.clone();
        payload.provider_id = self.provider_id.clone();

        let bytes_to_sign = payload.to_signed_bytes()?;
        let sig = self.key_pair.sign(&bytes_to_sign);
        let signature_hex = hex::encode(sig.as_ref());

        Ok(SignedReceipt {
            payload,
            signature_hex,
            public_key_hex: self.public_key_hex.clone(),
            algorithm: ED25519_ALGORITHM_NAME.to_string(),
            key_id: self.key_id.clone(),
        })
    }

    /// Independently verify a signed receipt using a given public key hex.
    pub fn verify_signature(
        receipt: &SignedReceipt,
        expected_pubkey_hex: Option<&str>,
    ) -> Result<()> {
        if receipt.algorithm != ED25519_ALGORITHM_NAME {
            return Err(Error::VerificationFailed(format!(
                "Unsupported receipt signature algorithm: {}",
                receipt.algorithm
            )));
        }

        let pubkey_hex = expected_pubkey_hex.unwrap_or(&receipt.public_key_hex);
        let pubkey_bytes = hex::decode(pubkey_hex)
            .map_err(|e| Error::VerificationFailed(format!("Invalid public key hex: {}", e)))?;

        let sig_bytes = hex::decode(&receipt.signature_hex)
            .map_err(|e| Error::VerificationFailed(format!("Invalid signature hex: {}", e)))?;

        let bytes_to_verify = receipt.payload.to_signed_bytes()?;

        let peer_pub_key = signature::UnparsedPublicKey::new(&signature::ED25519, &pubkey_bytes);

        peer_pub_key
            .verify(&bytes_to_verify, &sig_bytes)
            .map_err(|_| {
                Error::VerificationFailed("Receipt Ed25519 signature is INVALID".to_string())
            })
    }
}
