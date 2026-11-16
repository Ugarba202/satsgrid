use crate::common::error::{Error, Result};
use crate::common::types::{PaymentHash, Satoshis};
use crate::node::lightning::invoice::Invoice;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Detailed, typed outcome of an authoritative Lightning invoice settlement lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettlementOutcome {
    /// Authoritative backend confirmation that the invoice has settled.
    ConfirmedSettled {
        preimage: Option<String>,
        settled_at: i64,
        amount_sats: Option<u64>,
    },
    /// Authoritative backend confirmation that the invoice is open and awaiting payment.
    ConfirmedPending { expires_at: Option<i64> },
    /// Authoritative backend confirmation that the invoice expired without settlement.
    ConfirmedExpired { expired_at: i64 },
    /// Authoritative backend confirmation that the invoice was canceled.
    ConfirmedCancelled { cancelled_at: i64 },
    /// Invoice is not known to the backend or lookup returned 404 / empty result.
    Unknown { details: String },
    /// Backend could not be reached (network timeout, connection refused, 5xx).
    BackendUnavailable { error: String },
    /// Returned payload had contradictory or unparseable data.
    InvalidOrConflictingEvidence { details: String },
}

#[async_trait]
pub trait LightningBackend: Send + Sync {
    async fn create_invoice(&self, amount_sats: Satoshis, memo: &str) -> Result<Invoice>;
    async fn is_invoice_settled(&self, payment_hash: &PaymentHash) -> Result<bool>;

    /// Authoritative settlement lookup returning a detailed, typed `SettlementOutcome`.
    async fn check_settlement(&self, payment_hash: &PaymentHash) -> Result<SettlementOutcome> {
        // Fallback default implementation using `is_invoice_settled`
        match self.is_invoice_settled(payment_hash).await {
            Ok(true) => Ok(SettlementOutcome::ConfirmedSettled {
                preimage: None,
                settled_at: chrono::Utc::now().timestamp(),
                amount_sats: None,
            }),
            Ok(false) => Ok(SettlementOutcome::ConfirmedPending { expires_at: None }),
            Err(e) => Ok(SettlementOutcome::BackendUnavailable {
                error: e.to_string(),
            }),
        }
    }

    async fn pay_invoice(&self, invoice: &str) -> Result<String> {
        let _ = invoice;
        Err(Error::Lightning(
            "Invoice payment is not supported by this Lightning backend".to_string(),
        ))
    }
}
