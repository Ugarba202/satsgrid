use crate::common::types::Satoshis;
use crate::config::schema::PricingConfig;
use serde_json::Value;

/// Thread-safe pricing calculator for inference requests.
///
/// Computes satoshi costs dynamically based on base per-request fees
/// and per-token rates for prompts and completions.
#[derive(Debug, Clone, Default)]
pub struct PricingCalculator {
    config: PricingConfig,
}

impl PricingCalculator {
    pub fn new(config: PricingConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &PricingConfig {
        &self.config
    }

    /// Calculate total cost in Satoshis for a given prompt and completion estimate.
    pub fn calculate_cost(
        &self,
        prompt_tokens: usize,
        estimated_completion_tokens: Option<usize>,
    ) -> Satoshis {
        let base_sats = self.config.default_price_sats.0;
        let prompt_sats = (prompt_tokens as u64).saturating_mul(self.config.sats_per_prompt_token);
        let completion_sats = (estimated_completion_tokens.unwrap_or(0) as u64)
            .saturating_mul(self.config.sats_per_completion_token);

        Satoshis(
            base_sats
                .saturating_add(prompt_sats)
                .saturating_add(completion_sats),
        )
    }

    /// Heuristic estimation of token count from raw text (~4 characters per token).
    pub fn estimate_tokens_from_text(text: &str) -> usize {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return 0;
        }
        // Standard rule of thumb: ~4 characters per token for English text
        trimmed.len().div_ceil(4)
    }

    /// Estimate total prompt token count from an OpenAI-compatible chat completion payload.
    pub fn estimate_prompt_tokens_from_payload(payload: &Value) -> usize {
        let mut total_tokens = 0;

        if let Some(messages) = payload.get("messages").and_then(|m| m.as_array()) {
            for msg in messages {
                if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                    total_tokens += Self::estimate_tokens_from_text(content);
                } else if let Some(content_array) = msg.get("content").and_then(|c| c.as_array()) {
                    for item in content_array {
                        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                            total_tokens += Self::estimate_tokens_from_text(text);
                        }
                    }
                }
            }
        }

        total_tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_pricing_flat_rate_only() {
        let config = PricingConfig {
            default_price_sats: Satoshis(15),
            sats_per_prompt_token: 0,
            sats_per_completion_token: 0,
        };
        let calc = PricingCalculator::new(config);

        // Regardless of tokens, base flat rate applies
        let cost = calc.calculate_cost(100, Some(50));
        assert_eq!(cost, Satoshis(15));
    }

    #[test]
    fn test_pricing_with_token_rates() {
        let config = PricingConfig {
            default_price_sats: Satoshis(10),
            sats_per_prompt_token: 2,
            sats_per_completion_token: 5,
        };
        let calc = PricingCalculator::new(config);

        // 10 base + (20 * 2) + (10 * 5) = 10 + 40 + 50 = 100 sats
        let cost = calc.calculate_cost(20, Some(10));
        assert_eq!(cost, Satoshis(100));
    }

    #[test]
    fn test_pricing_zero_cost_free_tier() {
        let config = PricingConfig {
            default_price_sats: Satoshis(0),
            sats_per_prompt_token: 0,
            sats_per_completion_token: 0,
        };
        let calc = PricingCalculator::new(config);

        let cost = calc.calculate_cost(500, Some(200));
        assert_eq!(cost, Satoshis(0));
    }

    #[test]
    fn test_estimate_tokens_from_text() {
        assert_eq!(PricingCalculator::estimate_tokens_from_text(""), 0);
        assert_eq!(PricingCalculator::estimate_tokens_from_text("    "), 0);
        // "Hello world" is 11 chars -> (11 + 3) / 4 = 3 tokens
        assert_eq!(
            PricingCalculator::estimate_tokens_from_text("Hello world"),
            3
        );
    }

    #[test]
    fn test_estimate_prompt_tokens_from_payload() {
        let payload = json!({
            "model": "llama3.2",
            "messages": [
                {"role": "system", "content": "You are a helpful assistant."},
                {"role": "user", "content": "Explain Bitcoin Lightning."}
            ]
        });

        let tokens = PricingCalculator::estimate_prompt_tokens_from_payload(&payload);
        assert!(tokens > 0);
        // System message is 28 chars -> 7 tokens. User is 26 chars -> 7 tokens. Total 14 tokens.
        assert_eq!(tokens, 14);
    }
}
