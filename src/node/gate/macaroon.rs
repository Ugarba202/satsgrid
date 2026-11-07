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

    pub fn to_predicate_string(&self) -> String {
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
            Caveat::Capability(ref cap) => Some(cap.as_str()),
            _ => None,
        }
    }

    pub fn expires_at(&self) -> Option<u64> {
        match self {
            Caveat::ExpiresAt(ts) => Some(*ts),
            _ => None,
        }
    }
}

/// A serialized or deserialized L402 Macaroon structure.
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

    /// Extract the expiration timestamp from the caveats if present.
    pub fn expires_at(&self) -> Option<u64> {
        self.caveats
            .iter()
            .find_map(|raw| Caveat::parse(raw).and_then(|c| c.expires_at()))
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

    /// Compute the initial HMAC signature for a payment hash identifier using the root key.
    fn compute_initial_signature(&self, identifier: &PaymentHash) -> Result<Vec<u8>> {
        let mut mac = HmacSha256::new_from_slice(&self.root_key)
            .map_err(|e| Error::Internal(format!("Invalid HMAC key: {}", e)))?;
        mac.update(identifier.0.as_bytes());
        Ok(mac.finalize().into_bytes().to_vec())
    }

    /// Chain an additional caveat to an existing HMAC signature.
    fn chain_caveat_signature(current_sig: &[u8], caveat_str: &str) -> Result<Vec<u8>> {
        let mut mac = HmacSha256::new_from_slice(current_sig)
            .map_err(|e| Error::Internal(format!("Invalid HMAC signature chaining: {}", e)))?;
        mac.update(caveat_str.as_bytes());
        Ok(mac.finalize().into_bytes().to_vec())
    }

    /// Mint a new Macaroon bound to a specific payment hash, optionally attaching first-party caveats.
    pub fn mint(&self, payment_hash: &PaymentHash, caveats: Vec<Caveat>) -> Result<Macaroon> {
        let mut current_sig = self.compute_initial_signature(payment_hash)?;
        let mut raw_caveats = Vec::with_capacity(caveats.len());

        for caveat in caveats {
            let predicate = caveat.to_predicate_string();
            current_sig = Self::chain_caveat_signature(&current_sig, &predicate)?;
            raw_caveats.push(predicate);
        }

        Ok(Macaroon {
            location: self.location.clone(),
            identifier: payment_hash.0.clone(),
            caveats: raw_caveats,
            signature: hex::encode(current_sig),
        })
    }

    /// Cryptographically verify the integrity of a macaroon and enforce its caveats.
    pub fn verify(
        &self,
        macaroon: &Macaroon,
        requested_model: Option<&str>,
        now_timestamp: u64,
    ) -> Result<()> {
        let payment_hash = macaroon.payment_hash();
        let mut expected_sig = self.compute_initial_signature(&payment_hash)?;

        for raw_caveat in &macaroon.caveats {
            expected_sig = Self::chain_caveat_signature(&expected_sig, raw_caveat)?;

            if let Some(caveat) = Caveat::parse(raw_caveat) {
                match caveat {
                    Caveat::ExpiresAt(expires_at) => {
                        if now_timestamp > expires_at {
                            return Err(Error::VerificationFailed(format!(
                                "Macaroon has expired (expiry: {}, current: {})",
                                expires_at, now_timestamp
                            )));
                        }
                    }
                    Caveat::Model(ref allowed_model) => {
                        if let Some(req_m) = requested_model {
                            if req_m != allowed_model {
                                return Err(Error::VerificationFailed(format!(
                                    "Model '{}' is not permitted by this macaroon (allowed: '{}')",
                                    req_m, allowed_model
                                )));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        let expected_sig_hex = hex::encode(expected_sig);
        if expected_sig_hex.to_lowercase() != macaroon.signature.to_lowercase() {
            return Err(Error::VerificationFailed(
                "Invalid macaroon signature".to_string(),
            ));
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
        let payment_hash = PaymentHash("aabbccddeeff00112233445566778899".to_string());
        let caveats = vec![Caveat::ExpiresAt(2000000000)];

        let macaroon = service.mint(&payment_hash, caveats).unwrap();
        assert_eq!(macaroon.identifier, payment_hash.0);
        assert_eq!(macaroon.caveats.len(), 1);
        assert_eq!(macaroon.expires_at(), Some(2000000000));

        let res = service.verify(&macaroon, None, 1700000000);
        assert!(res.is_ok());
    }

    #[test]
    fn test_expired_macaroon_is_rejected() {
        let service = test_service();
        let payment_hash = PaymentHash("aabbccddeeff00112233445566778899".to_string());
        let caveats = vec![Caveat::ExpiresAt(1700000000)];

        let macaroon = service.mint(&payment_hash, caveats).unwrap();
        let res = service.verify(&macaroon, None, 1700000001);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("expired"));
    }

    #[test]
    fn test_model_restriction_caveat_enforced() {
        let service = test_service();
        let payment_hash = PaymentHash("aabbccddeeff00112233445566778899".to_string());
        let caveats = vec![Caveat::Model("llama3.2".to_string())];

        let macaroon = service.mint(&payment_hash, caveats).unwrap();

        assert!(service.verify(&macaroon, Some("llama3.2"), 1000).is_ok());
        assert!(service.verify(&macaroon, Some("mistral"), 1000).is_err());
    }

    #[test]
    fn test_tampered_signature_is_rejected() {
        let service = test_service();
        let payment_hash = PaymentHash("aabbccddeeff00112233445566778899".to_string());
        let mut macaroon = service.mint(&payment_hash, vec![]).unwrap();

        macaroon.signature = hex::encode([0x99u8; 32]);
        assert!(service.verify(&macaroon, None, 1000).is_err());
    }

    #[test]
    fn test_tampered_caveats_are_rejected() {
        let service = test_service();
        let payment_hash = PaymentHash("aabbccddeeff00112233445566778899".to_string());
        let mut macaroon = service
            .mint(&payment_hash, vec![Caveat::ExpiresAt(2000000000)])
            .unwrap();

        macaroon.caveats.push("model = evil_model".to_string());
        assert!(service.verify(&macaroon, None, 1000).is_err());
    }

    #[test]
    fn test_tampered_identifier_is_rejected() {
        let service = test_service();
        let payment_hash = PaymentHash("aabbccddeeff00112233445566778899".to_string());
        let mut macaroon = service.mint(&payment_hash, vec![]).unwrap();

        macaroon.identifier = "forged_payment_hash".to_string();
        assert!(service.verify(&macaroon, None, 1000).is_err());
    }

    #[test]
    fn test_base64_serialization_roundtrip() {
        let service = test_service();
        let payment_hash = PaymentHash("00112233445566778899aabbccddeeff".to_string());
        let macaroon = service
            .mint(
                &payment_hash,
                vec![Caveat::Capability("inference".to_string())],
            )
            .unwrap();

        let b64 = macaroon.to_base64().unwrap();
        let deserialized = Macaroon::from_base64(&b64).unwrap();

        assert_eq!(macaroon, deserialized);
        assert_eq!(deserialized.capability(), Some("inference".to_string()));
    }
}
