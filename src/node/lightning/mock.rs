use crate::common::error::{Error, Result};
use crate::common::types::{PaymentHash, Satoshis};
use crate::node::lightning::backend::LightningBackend;
use crate::node::lightning::invoice::Invoice;
use async_trait::async_trait;
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Debug)]
pub struct MockInvoiceState {
    pub preimage: String,
    pub settled: bool,
}

#[derive(Clone)]
pub struct MockLightningBackend {
    invoices: Arc<Mutex<HashMap<String, MockInvoiceState>>>,
}

impl Default for MockLightningBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MockLightningBackend {
    pub fn new() -> Self {
        Self {
            invoices: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn simulate_payment(&self, payment_hash: &PaymentHash) {
        let mut lock = self.invoices.lock().await;
        let entry = lock
            .entry(payment_hash.0.clone())
            .or_insert_with(|| MockInvoiceState {
                preimage: "0000000000000000000000000000000000000000000000000000000000000000"
                    .to_string(),
                settled: false,
            });
        entry.settled = true;
    }

    fn parse_mock_payment_hash(invoice: &str) -> Option<String> {
        // Find where "mock" is in the invoice
        if let Some(idx) = invoice.find("mock") {
            // The payment hash starts after "mock"
            let hash = &invoice[idx + 4..];
            if hash.len() == 64 {
                return Some(hash.to_string());
            }
        }
        None
    }

    pub async fn pay_invoice(&self, invoice: &str) -> Result<String> {
        let payment_hash = Self::parse_mock_payment_hash(invoice)
            .ok_or_else(|| Error::Lightning("Invalid mock invoice".to_string()))?;

        let mut invoices = self.invoices.lock().await;

        let invoice_state = invoices
            .get_mut(&payment_hash)
            .ok_or_else(|| Error::Lightning("Unknown mock invoice".to_string()))?;

        invoice_state.settled = true;

        Ok(invoice_state.preimage.clone())
    }
}

#[async_trait]
impl LightningBackend for MockLightningBackend {
    async fn pay_invoice(&self, invoice: &str) -> Result<String> {
        self.pay_invoice(invoice).await
    }

    async fn create_invoice(&self, amount_sats: Satoshis, _memo: &str) -> Result<Invoice> {
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        let preimage_str = hex::encode(key);

        let mut hasher = Sha256::new();
        hasher.update(key);
        let payment_hash_str = hex::encode(hasher.finalize());
        let invoice_str = format!("lnbc{}mock{}", amount_sats.0, payment_hash_str);

        let mut lock = self.invoices.lock().await;
        lock.insert(
            payment_hash_str.clone(),
            MockInvoiceState {
                preimage: preimage_str,
                settled: false,
            },
        );

        Ok(Invoice {
            bolt11: invoice_str,
            payment_hash: PaymentHash(payment_hash_str),
            amount: amount_sats,
        })
    }

    async fn is_invoice_settled(&self, payment_hash: &PaymentHash) -> Result<bool> {
        let invoices = self.invoices.lock().await;
        if let Some(state) = invoices.get(&payment_hash.0) {
            Ok(state.settled)
        } else {
            Ok(false)
        }
    }
}
