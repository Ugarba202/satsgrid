use crate::common::error::{Error, Result};
use crate::node::gate::macaroon::Macaroon;
use crate::node::lightning::invoice::Invoice;

/// Represents an L402 payment challenge returned with HTTP status 402 Payment Required.
///
/// Contains the base64-encoded macaroon credential token and the BOLT11 Lightning invoice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L402Challenge {
    pub macaroon: String,
    pub invoice: String,
}

impl L402Challenge {
    pub fn new(macaroon: impl Into<String>, invoice: impl Into<String>) -> Self {
        Self {
            macaroon: macaroon.into(),
            invoice: invoice.into(),
        }
    }

    /// Construct a challenge from a minted `Macaroon` and generated `Invoice`.
    pub fn from_components(macaroon: &Macaroon, invoice: &Invoice) -> Result<Self> {
        let token = macaroon.to_base64()?;
        Ok(Self {
            macaroon: token,
            invoice: invoice.bolt11.clone(),
        })
    }

    /// Format this challenge as a standard `WWW-Authenticate` header value.
    ///
    /// Example: `L402 token="AGFk...", invoice="lnbc100n..."`
    pub fn to_header_value(&self) -> String {
        format!(
            "L402 token=\"{}\", invoice=\"{}\"",
            self.macaroon, self.invoice
        )
    }

    /// Parse an incoming `WWW-Authenticate` header value into an `L402Challenge`.
    ///
    /// Supports both modern `L402` prefix and legacy `LSAT` prefix, in any attribute order.
    pub fn from_header_value(header: &str) -> Result<Self> {
        let trimmed = header.trim();
        let payload = if let Some(rest) = trimmed.strip_prefix("L402 ") {
            rest
        } else if let Some(rest) = trimmed.strip_prefix("LSAT ") {
            rest
        } else {
            return Err(Error::VerificationFailed(
                "Challenge header missing L402 or LSAT prefix".to_string(),
            ));
        };

        let mut token = None;
        let mut invoice = None;

        for part in payload.split(',') {
            let part = part.trim();
            if let Some(rest) = part.strip_prefix("token=\"") {
                if let Some(val) = rest.strip_suffix('\"') {
                    token = Some(val.to_string());
                }
            } else if let Some(rest) = part.strip_prefix("macaroon=\"") {
                if let Some(val) = rest.strip_suffix('\"') {
                    token = Some(val.to_string());
                }
            } else if let Some(rest) = part.strip_prefix("invoice=\"") {
                if let Some(val) = rest.strip_suffix('\"') {
                    invoice = Some(val.to_string());
                }
            }
        }

        let token = token.ok_or_else(|| {
            Error::VerificationFailed("Challenge header missing token parameter".to_string())
        })?;
        let invoice = invoice.ok_or_else(|| {
            Error::VerificationFailed("Challenge header missing invoice parameter".to_string())
        })?;

        Ok(Self {
            macaroon: token,
            invoice,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_challenge_to_header_value() {
        let challenge = L402Challenge::new("mock_token_b64", "lnbc100n1mockinvoice");
        let header = challenge.to_header_value();
        assert_eq!(
            header,
            "L402 token=\"mock_token_b64\", invoice=\"lnbc100n1mockinvoice\""
        );
    }

    #[test]
    fn test_challenge_from_header_value_standard() {
        let header = "L402 token=\"my_token\", invoice=\"my_invoice\"";
        let parsed = L402Challenge::from_header_value(header).expect("should parse standard L402");
        assert_eq!(parsed.macaroon, "my_token");
        assert_eq!(parsed.invoice, "my_invoice");
    }

    #[test]
    fn test_challenge_from_header_value_lsat_compatibility() {
        let header = "LSAT macaroon=\"legacy_token\", invoice=\"lnbc_invoice\"";
        let parsed = L402Challenge::from_header_value(header).expect("should parse legacy LSAT");
        assert_eq!(parsed.macaroon, "legacy_token");
        assert_eq!(parsed.invoice, "lnbc_invoice");
    }

    #[test]
    fn test_challenge_from_header_value_reversed_order() {
        let header = "L402 invoice=\"lnbc123\", token=\"token456\"";
        let parsed = L402Challenge::from_header_value(header).expect("should parse reversed order");
        assert_eq!(parsed.macaroon, "token456");
        assert_eq!(parsed.invoice, "lnbc123");
    }

    #[test]
    fn test_challenge_missing_token_errors() {
        let header = "L402 invoice=\"lnbc123\"";
        let result = L402Challenge::from_header_value(header);
        assert!(result.is_err());
    }

    #[test]
    fn test_challenge_missing_invoice_errors() {
        let header = "L402 token=\"token123\"";
        let result = L402Challenge::from_header_value(header);
        assert!(result.is_err());
    }

    #[test]
    fn test_challenge_missing_prefix_errors() {
        let header = "Bearer token=\"token123\", invoice=\"lnbc123\"";
        let result = L402Challenge::from_header_value(header);
        assert!(result.is_err());
    }

    #[test]
    fn test_challenge_roundtrip() {
        let original = L402Challenge::new("tok_abc123", "inv_xyz789");
        let header = original.to_header_value();
        let parsed = L402Challenge::from_header_value(&header).expect("roundtrip should succeed");
        assert_eq!(original, parsed);
    }
}
