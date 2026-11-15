import test from "node:test";
import assert from "node:assert/strict";
import {
  estimateTokensFromText,
  estimateInferenceCost,
} from "../src/lib/pricing/estimate";
import type { ModelEntry } from "../src/lib/api/node";

test("Token Estimation - empty and whitespace text return 0 tokens", () => {
  assert.equal(estimateTokensFromText(""), 0);
  assert.equal(estimateTokensFromText("   "), 0);
  assert.equal(estimateTokensFromText("\n\t  "), 0);
});

test("Token Estimation - matches Rust backend div_ceil(4) formula exactly", () => {
  // 1 to 4 chars -> 1 token
  assert.equal(estimateTokensFromText("a"), 1);
  assert.equal(estimateTokensFromText("abcd"), 1);
  // 5 chars -> 2 tokens
  assert.equal(estimateTokensFromText("abcde"), 2);
  // 8 chars -> 2 tokens
  assert.equal(estimateTokensFromText("abcdefgh"), 2);
  // 9 chars -> 3 tokens
  assert.equal(estimateTokensFromText("abcdefghi"), 3);
  // 100 chars -> 25 tokens
  assert.equal(estimateTokensFromText("a".repeat(100)), 25);
  // 101 chars -> 26 tokens
  assert.equal(estimateTokensFromText("a".repeat(101)), 26);
});

test("Cost Estimator - global fallback pricing with zero token rates", () => {
  const result = estimateInferenceCost({
    text: "Hello world, what is Lightning Network?",
    modelId: "llama3.2",
    modelEntry: null,
  });

  assert.equal(result.rates.flatRateSats, 10);
  assert.equal(result.rates.satsPerPromptToken, 0);
  assert.equal(result.rates.satsPerCompletionToken, 0);
  assert.equal(result.rates.isModelSpecific, false);
  assert.equal(result.minEstimatedSats, 10);
  assert.equal(result.maxEstimatedSats, 10);
});

test("Cost Estimator - model-specific pricing override takes precedence", () => {
  const modelWithOverrides: ModelEntry = {
    id: "deepseek-coder",
    object: "model",
    created: 1700000000,
    owned_by: "satsgrid",
    pricing: {
      flat_rate_sats: 5,
      sats_per_prompt_token: 1,
      sats_per_completion_token: 2,
    },
  };

  // 12 chars -> 3 tokens
  const result = estimateInferenceCost({
    text: "hello world!",
    modelId: "deepseek-coder",
    modelEntry: modelWithOverrides,
    maxTokens: 50,
  });

  assert.equal(result.estimatedPromptTokens, 3);
  assert.equal(result.rates.flatRateSats, 5);
  assert.equal(result.rates.satsPerPromptToken, 1);
  assert.equal(result.rates.satsPerCompletionToken, 2);
  assert.equal(result.rates.isModelSpecific, true);

  // min = 5 base + 3 tokens * 1 sat = 8 sats
  assert.equal(result.minEstimatedSats, 8);
  // max = 8 + 50 max_tokens * 2 sats = 108 sats
  assert.equal(result.maxEstimatedSats, 108);
});

test("Cost Estimator - missing pricing safely defaults to fallback", () => {
  const emptyModel: ModelEntry = {
    id: "unknown-model",
    object: "model",
    created: 1700000000,
    owned_by: "satsgrid",
    pricing: null,
  };

  const result = estimateInferenceCost({
    text: "testing fallback",
    modelId: "unknown-model",
    modelEntry: emptyModel,
    globalFallbackPricing: {
      default_price_sats: 15,
      sats_per_prompt_token: 0,
      sats_per_completion_token: 0,
    },
  });

  assert.equal(result.rates.flatRateSats, 15);
  assert.equal(result.minEstimatedSats, 15);
  assert.equal(result.maxEstimatedSats, 15);
});
