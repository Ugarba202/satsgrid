use crate::common::error::{Error, Result};
use crate::common::types::{PaymentHash, Preimage};
use crate::node::gate::macaroon::{Macaroon, MacaroonService};
use crate::node::lightning::backend::LightningBackend;
use sha2::{Digest, Sha256};

/// Parsed L402 credentials extracted from the HTTP `Authorization` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L402Credentials {
    pub macaroon: Macaroon,
    pub preimage: Preimage,
}

impl L402Credentials {
    /// Parse an incoming HTTP `Authorization` header value.
    ///
    /// Expected format: `L402 <macaroon_base64>:<preimage_hex>`
    /// Also supports legacy `LSAT` prefix.
    pub fn from_header_value(header: &str) -> Result<Self> {
        let trimmed = header.trim();
        let payload = if let Some(rest) = trimmed.strip_prefix("L402 ") {
            rest
        } else if let Some(rest) = trimmed.strip_prefix("LSAT ") {
            rest
        } else {
            return Err(Error::VerificationFailed(
                "Authorization header missing L402 or LSAT prefix".to_string(),
            ));
        };

        let (token_str, preimage_str) = payload.split_once(':').ok_or_else(|| {
            Error::VerificationFailed(
                "Invalid L402 credentials format; expected '<macaroon>:<preimage>'".to_string(),
            )
        })?;

        let macaroon = Macaroon::from_base64(token_str.trim())?;
        let preimage = Preimage(preimage_str.trim().to_string());

        Ok(Self { macaroon, preimage })
    }
}

/// Verifies L402 credentials by evaluating cryptographic preimage hashes,
/// macaroon caveats, and Lightning Network invoice settlement.
pub struct L402Verifier;

impl L402Verifier {
    /// Compute the SHA-256 hash of a hex-encoded preimage string.
    pub fn hash_preimage(preimage_hex: &str) -> Result<PaymentHash> {
        let preimage_bytes = hex::decode(preimage_hex.trim())
            .map_err(|e| Error::VerificationFailed(format!("Preimage is not valid hex: {}", e)))?;

        let mut hasher = Sha256::new();
        hasher.update(&preimage_bytes);
        let hash_bytes = hasher.finalize();

        Ok(PaymentHash(hex::encode(hash_bytes)))
    }

    /// Fully verify L402 credentials against the server root key and Lightning backend.
    pub async fn verify_credentials<B: LightningBackend + ?Sized>(
        macaroon_service: &MacaroonService,
        lightning_backend: &B,
        credentials: &L402Credentials,
        requested_model: Option<&str>,
        now_timestamp: u64,
    ) -> Result<()> {
        // 1. Cryptographically verify that sha256(preimage) matches macaroon identifier
        let computed_hash = Self::hash_preimage(&credentials.preimage.0)?;
        if computed_hash.0.to_lowercase() != credentials.macaroon.identifier.to_lowercase() {
            return Err(Error::VerificationFailed(format!(
                "Preimage hash mismatch: computed '{}', macaroon identifier was '{}'",
                computed_hash.0, credentials.macaroon.identifier
            )));
        }

        // 2. Verify macaroon signature and caveats (expiration, model restriction)
        macaroon_service.verify(&credentials.macaroon, requested_model, now_timestamp)?;

        // 3. Verify that the invoice was settled on the Lightning backend
        let is_settled = lightning_backend
            .is_invoice_settled(&credentials.macaroon.payment_hash())
            .await?;

        if !is_settled {
            return Err(Error::VerificationFailed(
                "Lightning invoice is not settled".to_string(),
            ));
        }

        Ok(())
    }

    /// Convenience helper to parse and verify directly from the HTTP `Authorization` header.
    pub async fn verify_header<B: LightningBackend + ?Sized>(
        macaroon_service: &MacaroonService,
        lightning_backend: &B,
        auth_header: &str,
        requested_model: Option<&str>,
        now_timestamp: u64,
    ) -> Result<L402Credentials> {
        let credentials = L402Credentials::from_header_value(auth_header)?;
        Self::verify_credentials(
            macaroon_service,
            lightning_backend,
            &credentials,
            requested_model,
            now_timestamp,
        )
        .await?;
        Ok(credentials)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::gate::macaroon::Caveat;
    use crate::node::lightning::mock::MockLightningBackend;

    fn test_fixtures() -> (MacaroonService, MockLightningBackend) {
        let service = MacaroonService::new(
            b"test-secret-root-key-32-bytes-ok".to_vec(),
            "infernos-node",
        );
        let backend = MockLightningBackend::new();
        (service, backend)
    }

    #[test]
    fn test_parse_authorization_header_valid_l402() {
        let (service, _) = test_fixtures();
        let hash = PaymentHash("11223344556677889900aabbccddeeff".to_string());
        let macaroon = service.mint(&hash, vec![]).unwrap();
        let token = macaroon.to_base64().unwrap();
        let header = format!("L402 {}:00112233445566778899aabbccddeeff", token);
        let creds = L402Credentials::from_header_value(&header);
        assert!(creds.is_ok());
    }

    #[test]
    fn test_parse_authorization_header_valid_lsat() {
        let (service, _) = test_fixtures();
        let hash = PaymentHash("11223344556677889900aabbccddeeff".to_string());
        let macaroon = service.mint(&hash, vec![]).unwrap();
        let token = macaroon.to_base64().unwrap();
        let header = format!("LSAT {}:00112233445566778899aabbccddeeff", token);
        let creds = L402Credentials::from_header_value(&header);
        assert!(creds.is_ok());
    }

    #[test]
    fn test_parse_authorization_header_missing_colon_errors() {
        let header = "L402 nocolonseparatedhere";
        let creds = L402Credentials::from_header_value(header);
        assert!(creds.is_err());
    }

    #[test]
    fn test_parse_authorization_header_invalid_prefix_errors() {
        let header = "Bearer some_token";
        let creds = L402Credentials::from_header_value(header);
        assert!(creds.is_err());
    }

    #[tokio::test]
    async fn test_verify_valid_payment_success() {
        let (service, backend) = test_fixtures();

        // 1. Derive known preimage for mock testing (32 bytes hex)
        let preimage_bytes = [0x42u8; 32];
        let preimage_hex = hex::encode(preimage_bytes);

        // Preimage hash
        let hash = L402Verifier::hash_preimage(&preimage_hex).unwrap();

        // Simulate payment settled in backend
        backend.simulate_payment(&hash).await;

        // 3. Mint macaroon bound to this hash
        let macaroon = service
            .mint(
                &hash,
                vec![
                    Caveat::ExpiresAt(2000000000),
                    Caveat::Model("llama3".to_string()),
                ],
            )
            .unwrap();

        let creds = L402Credentials {
            macaroon,
            preimage: Preimage(preimage_hex),
        };

        // 4. Verification should succeed
        let result = L402Verifier::verify_credentials(
            &service,
            &backend,
            &creds,
            Some("llama3"),
            1700000000,
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_verify_preimage_hash_mismatch_rejected() {
        let (service, backend) = test_fixtures();

        // Real hash
        let hash = PaymentHash("11223344556677889900aabbccddeeff".to_string());
        let macaroon = service.mint(&hash, vec![]).unwrap();

        // Wrong preimage
        let wrong_preimage = Preimage(hex::encode([0x01u8; 32]));

        let creds = L402Credentials {
            macaroon,
            preimage: wrong_preimage,
        };

        let result =
            L402Verifier::verify_credentials(&service, &backend, &creds, None, 1700000000).await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("hash mismatch"));
    }

    #[tokio::test]
    async fn test_verify_unsettled_invoice_rejected() {
        let (service, backend) = test_fixtures();

        let preimage_bytes = [0x42u8; 32];
        let preimage_hex = hex::encode(preimage_bytes);
        let hash = L402Verifier::hash_preimage(&preimage_hex).unwrap();

        // Do NOT simulate payment in backend (invoice remains unsettled)
        let macaroon = service.mint(&hash, vec![]).unwrap();

        let creds = L402Credentials {
            macaroon,
            preimage: Preimage(preimage_hex),
        };

        let result =
            L402Verifier::verify_credentials(&service, &backend, &creds, None, 1700000000).await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not settled"));
    }

    #[tokio::test]
    async fn test_verify_expired_macaroon_rejected() {
        let (service, backend) = test_fixtures();

        let preimage_bytes = [0x42u8; 32];
        let preimage_hex = hex::encode(preimage_bytes);
        let hash = L402Verifier::hash_preimage(&preimage_hex).unwrap();
        backend.simulate_payment(&hash).await;

        let macaroon = service
            .mint(&hash, vec![Caveat::ExpiresAt(1700000000)])
            .unwrap();

        let creds = L402Credentials {
            macaroon,
            preimage: Preimage(preimage_hex),
        };

        // Verifying at 1700000001 (1 second past expiry)
        let result =
            L402Verifier::verify_credentials(&service, &backend, &creds, None, 1700000001).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("expired"));
    }

    #[tokio::test]
    async fn test_verify_disallowed_model_rejected() {
        let (service, backend) = test_fixtures();

        let preimage_bytes = [0x42u8; 32];
        let preimage_hex = hex::encode(preimage_bytes);
        let hash = L402Verifier::hash_preimage(&preimage_hex).unwrap();
        backend.simulate_payment(&hash).await;

        let macaroon = service
            .mint(&hash, vec![Caveat::Model("llama3".to_string())])
            .unwrap();

        let creds = L402Credentials {
            macaroon,
            preimage: Preimage(preimage_hex),
        };

        // Requesting "mistral" must be rejected
        let result = L402Verifier::verify_credentials(
            &service,
            &backend,
            &creds,
            Some("mistral"),
            1700000000,
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not permitted"));
    }
}
