use crate::common::error::{Error, Result};
use crate::common::types::PaymentHash;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// First-party caveat attached to an L402 macaroon credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Caveat {
    /// Token expires at the given Unix timestamp in seconds.
    ExpiresAt(u64),
    /// Token is restricted to querying a specific AI model.
    Model(String),
    /// Token is tied to a specific session UUID.
    Session(String),
    /// Maximum Satoshis budget allocated for this token.
    Budget(u64),
    Capability(String),
    /// Custom key-value predicate.
    Custom {
        key: String,
        value: String,
    },
}

impl Caveat {
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if let Some(rest) = trimmed.strip_prefix("time < ") {
            return rest.trim().parse::<u64>().ok().map(Caveat::ExpiresAt);
        }
        if let Some(rest) = trimmed.strip_prefix("model = ") {
            return Some(Caveat::Model(rest.trim().to_string()));
        }
        if let Some(rest) = trimmed.strip_prefix("session = ") {
            return Some(Caveat::Session(rest.trim().to_string()));
        }
        if let Some(rest) = trimmed.strip_prefix("budget = ") {
            return rest.trim().parse::<u64>().ok().map(Caveat::Budget);
        }
        if let Some(rest) = trimmed.strip_prefix("capability = ") {
            return Some(Caveat::Capability(rest.trim().to_string()));
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            return Some(Caveat::Custom {
                key: k.trim().to_string(),
                value: v.trim().to_string(),
            });
        }
        None
    }

    pub fn to_predicate(&self) -> String {
        match self {
            Caveat::ExpiresAt(ts) => format!("time < {}", ts),
            Caveat::Model(m) => format!("model = {}", m),
            Caveat::Session(s) => format!("session = {}", s),
            Caveat::Budget(b) => format!("budget = {}", b),
            Caveat::Capability(c) => format!("capability = {}", c),
            Caveat::Custom { key, value } => format!("{} = {}", key, value),
        }
    }

    pub fn capability(&self) -> Option<&str> {
        match self {
            Caveat::Capability(value) => Some(value.as_str()),
            _ => None,
        }
    }
}

/// An L402 Macaroon containing identifier, location, caveats, and HMAC signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Macaroon {
    pub location: String,
    pub identifier: String,
    pub caveats: Vec<String>,
    pub signature: String,
}

impl Macaroon {
    /// Serialize this macaroon to a standard Base64 string for HTTP headers.
    pub fn to_base64(&self) -> Result<String> {
        let json = serde_json::to_vec(self)
            .map_err(|e| Error::Internal(format!("Failed to serialize macaroon: {}", e)))?;
        Ok(BASE64.encode(json))
    }

    /// Deserialize a macaroon from a Base64-encoded string.
    pub fn from_base64(encoded: &str) -> Result<Self> {
        let bytes = BASE64
            .decode(encoded.trim())
            .map_err(|e| Error::VerificationFailed(format!("Invalid base64 macaroon: {}", e)))?;
        let macaroon: Macaroon = serde_json::from_slice(&bytes)
            .map_err(|e| Error::VerificationFailed(format!("Invalid macaroon payload: {}", e)))?;
        Ok(macaroon)
    }

    /// Get the associated payment hash as a strongly-typed `PaymentHash`.
    pub fn payment_hash(&self) -> PaymentHash {
        PaymentHash(self.identifier.clone())
    }

    /// Extract the capability from the caveats if present.
    pub fn capability(&self) -> Option<String> {
        self.caveats
            .iter()
            .find_map(|raw| Caveat::parse(raw).and_then(|c| c.capability().map(String::from)))
    }
}

/// Service responsible for minting and verifying L402 macaroons.
#[derive(Clone)]
pub struct MacaroonService {
    root_key: Vec<u8>,
    location: String,
}

impl MacaroonService {
    pub fn new(root_key: Vec<u8>, location: impl Into<String>) -> Self {
        Self {
            root_key,
            location: location.into(),
        }
    }

    pub fn root_key(&self) -> &[u8] {
        &self.root_key
    }

    pub fn location(&self) -> &str {
        &self.location
    }

    /// Compute the HMAC-SHA256 signature chain for an identifier and list of caveats.
    fn compute_signature(&self, identifier: &str, caveats: &[String]) -> Result<Vec<u8>> {
        let mut mac = HmacSha256::new_from_slice(&self.root_key)
            .map_err(|e| Error::Internal(format!("Invalid HMAC root key: {}", e)))?;
        mac.update(identifier.as_bytes());
        let mut current_sig = mac.finalize().into_bytes().to_vec();

        for caveat in caveats {
            let mut step_mac = HmacSha256::new_from_slice(&current_sig)
                .map_err(|e| Error::Internal(format!("Failed HMAC chaining: {}", e)))?;
            step_mac.update(caveat.as_bytes());
            current_sig = step_mac.finalize().into_bytes().to_vec();
        }

        Ok(current_sig)
    }

    /// Mint a new signed macaroon bound to a payment hash with optional caveats.
    pub fn mint(&self, payment_hash: &PaymentHash, caveats: Vec<Caveat>) -> Result<Macaroon> {
        let caveat_strings: Vec<String> = caveats.iter().map(|c| c.to_predicate()).collect();
        let sig_bytes = self.compute_signature(&payment_hash.0, &caveat_strings)?;
        let signature = hex::encode(sig_bytes);

        Ok(Macaroon {
            location: self.location.clone(),
            identifier: payment_hash.0.clone(),
            caveats: caveat_strings,
            signature,
        })
    }

    /// Verify a macaroon's cryptographic signature and evaluate all embedded caveats.
    pub fn verify(
        &self,
        macaroon: &Macaroon,
        requested_model: Option<&str>,
        now_timestamp: u64,
    ) -> Result<()> {
        // 1. Verify cryptographic signature integrity
        let expected_sig_bytes = self.compute_signature(&macaroon.identifier, &macaroon.caveats)?;
        let expected_sig_hex = hex::encode(expected_sig_bytes);

        if macaroon.signature != expected_sig_hex {
            return Err(Error::VerificationFailed(
                "Macaroon signature mismatch or tampered token".to_string(),
            ));
        }

        // 2. Evaluate caveats
        for raw_caveat in &macaroon.caveats {
            match Caveat::parse(raw_caveat) {
                Some(Caveat::ExpiresAt(expires_at)) => {
                    if now_timestamp >= expires_at {
                        return Err(Error::VerificationFailed(format!(
                            "Macaroon has expired (expiry: {}, current: {})",
                            expires_at, now_timestamp
                        )));
                    }
                }
                Some(Caveat::Model(allowed_model)) => {
                    if let Some(req) = requested_model {
                        if req != allowed_model {
                            return Err(Error::VerificationFailed(format!(
                                "Model '{}' not permitted by macaroon (allowed: '{}')",
                                req, allowed_model
                            )));
                        }
                    }
                }
                _ => {
                    // Other caveats (session, budget) are checked by their respective managers
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_service() -> MacaroonService {
        MacaroonService::new(
            b"test-secret-root-key-32-bytes-ok".to_vec(),
            "infernos-node",
        )
    }

    #[test]
    fn test_mint_and_verify_valid_macaroon() {
        let service = test_service();
        let hash = PaymentHash("11223344556677889900aabbccddeeff".to_string());
        let caveats = vec![
            Caveat::ExpiresAt(2000000000),
            Caveat::Model("llama3".to_string()),
        ];

        let macaroon = service.mint(&hash, caveats).expect("mint should succeed");
        assert_eq!(macaroon.location, "infernos-node");
        assert_eq!(macaroon.identifier, hash.0);
        assert_eq!(macaroon.caveats.len(), 2);

        // Verification at timestamp 1700000000 for model llama3 should pass
        let result = service.verify(&macaroon, Some("llama3"), 1700000000);
        assert!(result.is_ok());
    }

    #[test]
    fn test_tampered_signature_is_rejected() {
        let service = test_service();
        let hash = PaymentHash("11223344556677889900aabbccddeeff".to_string());
        let mut macaroon = service.mint(&hash, vec![]).expect("mint should succeed");

        // Tamper with signature
        macaroon.signature = "deadbeef".to_string();
        let result = service.verify(&macaroon, None, 1700000000);
        assert!(result.is_err());
    }

    #[test]
    fn test_tampered_identifier_is_rejected() {
        let service = test_service();
        let hash = PaymentHash("11223344556677889900aabbccddeeff".to_string());
        let mut macaroon = service.mint(&hash, vec![]).expect("mint should succeed");

        // Tamper with payment hash identifier
        macaroon.identifier = "99999999999999999999999999999999".to_string();
        let result = service.verify(&macaroon, None, 1700000000);
        assert!(result.is_err());
    }

    #[test]
    fn test_tampered_caveats_are_rejected() {
        let service = test_service();
        let hash = PaymentHash("11223344556677889900aabbccddeeff".to_string());
        let mut macaroon = service
            .mint(&hash, vec![Caveat::Model("llama3".to_string())])
            .expect("mint should succeed");

        // Add an extra caveat without re-signing
        macaroon.caveats.push("time < 9999999999".to_string());
        let result = service.verify(&macaroon, Some("llama3"), 1700000000);
        assert!(result.is_err());
    }

    #[test]
    fn test_expired_macaroon_is_rejected() {
        let service = test_service();
        let hash = PaymentHash("11223344556677889900aabbccddeeff".to_string());
        let caveats = vec![Caveat::ExpiresAt(1700000000)];

        let macaroon = service.mint(&hash, caveats).expect("mint should succeed");
        // Verify at timestamp 1700000001 (1 second past expiry)
        let result = service.verify(&macaroon, None, 1700000001);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("expired"));
    }

    #[test]
    fn test_model_restriction_caveat_enforced() {
        let service = test_service();
        let hash = PaymentHash("11223344556677889900aabbccddeeff".to_string());
        let caveats = vec![Caveat::Model("llama3".to_string())];

        let macaroon = service.mint(&hash, caveats).expect("mint should succeed");

        // Asking for llama3 is OK
        assert!(service
            .verify(&macaroon, Some("llama3"), 1700000000)
            .is_ok());

        // Asking for mistral must be rejected
        let result = service.verify(&macaroon, Some("mistral"), 1700000000);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not permitted"));
    }

    #[test]
    fn test_base64_serialization_roundtrip() {
        let service = test_service();
        let hash = PaymentHash("aabbccddeeff".to_string());
        let macaroon = service
            .mint(&hash, vec![Caveat::Budget(500)])
            .expect("mint should succeed");

        let encoded = macaroon.to_base64().expect("serialization should succeed");
        let decoded = Macaroon::from_base64(&encoded).expect("deserialization should succeed");

        assert_eq!(macaroon, decoded);
    }
}
