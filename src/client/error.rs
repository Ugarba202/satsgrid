use thiserror::Error;

#[derive(Error, Debug)]
pub enum ClientError {
    #[error("Payment failed: {0}")]
    Payment(String),

    #[error("Budget exceeded: allocated {allocated} sats, needed {needed} sats")]
    BudgetExceeded { allocated: u64, needed: u64 },

    #[error("SatsGrid session expired or budget exhausted")]
    SessionExpired,

    #[error("HTTP request error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Malformed L402 challenge: {0}")]
    MalformedChallenge(String),

    #[error("Protocol error: {0}")]
    Protocol(String),
}
