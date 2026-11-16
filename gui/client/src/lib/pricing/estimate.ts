import type { ModelEntry, ModelPricing } from "@/lib/api/node";

export const DEFAULT_FALLBACK_PRICING = {
  default_price_sats: 10,
  sats_per_prompt_token: 0,
  sats_per_completion_token: 0,
};

export interface CostEstimateInput {
  text: string;
  modelId: string;
  modelEntry?: ModelEntry | null;
  globalFallbackPricing?: {
    default_price_sats?: number;
    sats_per_prompt_token?: number;
    sats_per_completion_token?: number;
  };
  maxTokens?: number;
}

export interface CostEstimateResult {
  /** Estimated prompt tokens using the backend ~4 chars/token heuristic */
  estimatedPromptTokens: number;
  /** Minimum estimated cost in Satoshis (flat fee + prompt tokens) */
  minEstimatedSats: number;
  /** Maximum estimated upfront liability hold (flat fee + prompt + max completion tokens) */
  maxEstimatedSats: number;
  /** Active pricing rates applied */
  rates: {
    flatRateSats: number;
    satsPerPromptToken: number;
    satsPerCompletionToken: number;
    isModelSpecific: boolean;
  };
  /** Explanation string */
  explanation: string;
}

/**
 * Heuristic estimation of token count from raw text (~4 characters per token).
 * Matches PricingCalculator::estimate_tokens_from_text in src/node/pricing.rs exactly.
 */
export function estimateTokensFromText(text: string): number {
  const trimmed = text.trim();
  if (trimmed.length === 0) {
    return 0;
  }
  return Math.ceil(trimmed.length / 4);
}

/**
 * Calculate inference cost estimate matching SatsGrid PayGuard / PricingCalculator logic.
 *
 * Source of truth:
 * Cost = flat_rate_sats + (prompt_tokens * sats_per_prompt_token) + (completion_tokens * sats_per_completion_token)
 */
export function estimateInferenceCost(input: CostEstimateInput): CostEstimateResult {
  const {
    text,
    modelEntry,
    globalFallbackPricing = DEFAULT_FALLBACK_PRICING,
    maxTokens,
  } = input;

  const promptTokens = estimateTokensFromText(text);

  // Model-specific pricing overrides take precedence over global node defaults
  const modelPricing: ModelPricing | undefined | null = modelEntry?.pricing;

  const hasModelFlatRate = typeof modelPricing?.flat_rate_sats === "number";
  const hasModelPromptRate = typeof modelPricing?.sats_per_prompt_token === "number";
  const hasModelCompletionRate = typeof modelPricing?.sats_per_completion_token === "number";

  const isModelSpecific = hasModelFlatRate || hasModelPromptRate || hasModelCompletionRate;

  const flatRateSats = Math.max(
    0,
    hasModelFlatRate
      ? modelPricing!.flat_rate_sats!
      : globalFallbackPricing.default_price_sats ?? DEFAULT_FALLBACK_PRICING.default_price_sats
  );

  const satsPerPromptToken = Math.max(
    0,
    hasModelPromptRate
      ? modelPricing!.sats_per_prompt_token!
      : globalFallbackPricing.sats_per_prompt_token ?? DEFAULT_FALLBACK_PRICING.sats_per_prompt_token
  );

  const satsPerCompletionToken = Math.max(
    0,
    hasModelCompletionRate
      ? modelPricing!.sats_per_completion_token!
      : globalFallbackPricing.sats_per_completion_token ?? DEFAULT_FALLBACK_PRICING.sats_per_completion_token
  );

  // Minimum cost: upfront flat fee + prompt tokens fee (0 completion tokens)
  const promptSats = promptTokens * satsPerPromptToken;
  const minEstimatedSats = flatRateSats + promptSats;

  // Maximum cost: flat fee + prompt tokens + max_tokens * completion rate
  // If maxTokens is not specified or 0, min and max are identical
  const completionTokensEstimate = typeof maxTokens === "number" && maxTokens > 0 ? maxTokens : 0;
  const completionSats = completionTokensEstimate * satsPerCompletionToken;
  const maxEstimatedSats = minEstimatedSats + completionSats;

  let explanation = `Estimated: ~${minEstimatedSats} sats`;
  if (maxEstimatedSats > minEstimatedSats) {
    explanation = `Estimated: ~${minEstimatedSats} - ${maxEstimatedSats} sats (based on max tokens)`;
  } else if (minEstimatedSats === flatRateSats && satsPerPromptToken === 0) {
    explanation = `Fixed base fee: ${flatRateSats} sats`;
  }

  return {
    estimatedPromptTokens: promptTokens,
    minEstimatedSats,
    maxEstimatedSats,
    rates: {
      flatRateSats,
      satsPerPromptToken,
      satsPerCompletionToken,
      isModelSpecific,
    },
    explanation,
  };
}
