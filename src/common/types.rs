use serde::{Deserialize, Serialize};

use uuid::Uuid;

/// Satoshis amount type wrapper
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct Satoshis(pub u64);

/// Model identifier
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ModelId(pub String);

/// Session identifier
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SessionId(pub Uuid);

/// 32-byte payment hash (hex-encoded string)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PaymentHash(pub String);

/// 32-byte preimage (hex-encoded string)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Preimage(pub String);

/// Token usage accounting
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}
