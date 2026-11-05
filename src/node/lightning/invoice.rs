use crate::common::types::{PaymentHash, Satoshis};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invoice {
    pub bolt11: String,
    pub payment_hash: PaymentHash,
    pub amount: Satoshis,
}

pub fn parse_payment_hash(_invoice: &str) -> Option<String> {
    None
}
