use crate::common::error::{Error, Result};
use crate::common::types::{PaymentHash, Satoshis};
use crate::node::lightning::invoice::Invoice;
use async_trait::async_trait;

#[async_trait]
pub trait LightningBackend: Send + Sync {
    async fn create_invoice(&self, amount_sats: Satoshis, memo: &str) -> Result<Invoice>;
    async fn is_invoice_settled(&self, payment_hash: &PaymentHash) -> Result<bool>;
    async fn pay_invoice(&self, invoice: &str) -> Result<String> {
        let _ = invoice;
        Err(Error::Lightning(
            "Invoice payment is not supported by this Lightning backend".to_string(),
        ))
    }
}
