use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Lightning error: {0}")]
    Lightning(String),

    #[error("Payment required: invoice={invoice}, token={token}")]
    PaymentRequired { invoice: String, token: String },

    #[error("Session required. Please call /v1/session/new to establish a payment session.")]
    SessionRequired,

    #[error("L402 verification failed: {0}")]
    VerificationFailed(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Forbidden: {0}")]
    Forbidden(String),

    #[error("Session budget exhausted: {0}")]
    BudgetExhausted(String),

    #[error("Upstream inference error: {0}")]
    Upstream(String),

    #[error("Internal error: {0}")]
    Internal(String),
}
