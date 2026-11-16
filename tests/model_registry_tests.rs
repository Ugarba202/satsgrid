use satsgrid::common::types::Satoshis;
use satsgrid::config::schema::PricingConfig;
use satsgrid::node::pricing::PricingCalculator;
use satsgrid::node::storage::model::ModelRecord;

#[test]
fn test_per_model_pricing_override_precedence() {
    let global_config = PricingConfig {
        default_price_sats: Satoshis(10),
        sats_per_prompt_token: 1,
        sats_per_completion_token: 2,
    };
    let calc = PricingCalculator::new(global_config);

    // 1. Without model record: uses global defaults
    // 10 base + (10 * 1) + (5 * 2) = 10 + 10 + 10 = 30
    assert_eq!(
        calc.calculate_cost_for_model(None, 10, Some(5)),
        Satoshis(30)
    );

    // 2. With model record with full overrides
    let custom_model = ModelRecord {
        id: "premium-llm".to_string(),
        name: "Premium LLM".to_string(),
        owned_by: "provider-1".to_string(),
        description: None,
        context_length: Some(8192),
        flat_rate_sats: Some(50),
        sats_per_prompt_token: Some(5),
        sats_per_completion_token: Some(10),
        is_enabled: true,
        is_available: true,
        created_at: 1000,
        updated_at: 1000,
    };

    // 50 base + (10 * 5) + (5 * 10) = 50 + 50 + 50 = 150
    assert_eq!(
        calc.calculate_cost_for_model(Some(&custom_model), 10, Some(5)),
        Satoshis(150)
    );

    // 3. With model record having partial overrides (e.g. flat rate only)
    let partial_model = ModelRecord {
        id: "flat-only".to_string(),
        name: "Flat Only".to_string(),
        owned_by: "provider-1".to_string(),
        description: None,
        context_length: Some(4096),
        flat_rate_sats: Some(25),
        sats_per_prompt_token: None,     // falls back to global 1
        sats_per_completion_token: None, // falls back to global 2
        is_enabled: true,
        is_available: true,
        created_at: 1000,
        updated_at: 1000,
    };

    // 25 base + (10 * 1) + (5 * 2) = 25 + 10 + 10 = 45
    assert_eq!(
        calc.calculate_cost_for_model(Some(&partial_model), 10, Some(5)),
        Satoshis(45)
    );
}
