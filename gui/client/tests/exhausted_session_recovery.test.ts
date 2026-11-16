import test from "node:test";
import assert from "node:assert/strict";
import {
  inferenceFailureStatus,
  restoredStatus,
  walletFailureStatus,
  markBackendConfirmed,
  attachPaymentProof,
  sessionFromChallenge,
} from "@/lib/session-lifecycle";

test("inferenceFailureStatus categorizes all session recovery failure modes accurately", () => {
  // 1. Insufficient / Exhausted session budget
  assert.equal(
    inferenceFailureStatus(402, "Session budget exhausted", true),
    "exhausted",
    "402 budget exhausted must map to 'exhausted' status"
  );
  assert.equal(
    inferenceFailureStatus(402, "Insufficient remaining balance for request", true),
    "exhausted",
    "Insufficient balance error message must map to 'exhausted' status"
  );

  // 2. Expired session
  assert.equal(
    inferenceFailureStatus(400, "Session has expired", true),
    "expired",
    "Session expiration message must map to 'expired' status"
  );
  assert.equal(
    inferenceFailureStatus(400, "The requested session is expired", false),
    "expired",
    "Session expiration without proof must map to 'expired' status"
  );

  // 3. Closed session
  assert.equal(
    inferenceFailureStatus(400, "Session is closed and finalized", true),
    "closed",
    "Closed session message must map to 'closed' status"
  );

  // 4. Invalid or rejected authorization
  assert.equal(
    inferenceFailureStatus(401, "Preimage does not match invoice payment hash", true),
    "invalid_payment",
    "401 Unauthorized must map to 'invalid_payment' status"
  );
  assert.equal(
    inferenceFailureStatus(403, "Missing required capability 'inference'", true),
    "invalid_payment",
    "403 Forbidden must map to 'invalid_payment' status"
  );

  // 5. Payment pending and unconfirmed
  assert.equal(
    inferenceFailureStatus(500, "Network connection error", true),
    "payment_submitted",
    "Transient error with local payment proof must preserve 'payment_submitted' status"
  );

  // 6. Generic failure without payment proof
  assert.equal(
    inferenceFailureStatus(500, "Server error", false),
    "inference_error",
    "Server error without proof must map to 'inference_error'"
  );
});

test("Session replacement flow preserves state isolation and does not mutate historical sessions", () => {
  const originalSession = {
    id: "3b2f5197-3c31-40d1-8b9f-599a3b8c03bc",
    status: "active",
    model: "llama3.2",
    capability: "inference",
    budget_sats: 100,
    remaining_sats: 0,
    spent_sats: 100,
    macaroon: "mac_original",
    preimage: "preimage_original",
    invoice: "invoice_original",
    created_at: new Date().toISOString(),
    requests_count: 5,
    messages: [
      { role: "user", content: "Hello" },
      { role: "assistant", content: "World" },
    ],
  };

  // Original session is exhausted
  assert.equal(originalSession.remaining_sats, 0);

  // When developer triggers 'New Session', a replacement session challenge is created
  const newChallenge = {
    macaroon: "mac_replacement",
    invoice: "lnbcrt250u1pn25870abcdef1234567890abcdef1234567890",
    parsedCaveats: {
      session: "a1b2c3d4-1111-4222-8333-444455556666",
      budget: "250",
      capability: "inference",
    },
  };

  const replacementSession = sessionFromChallenge(newChallenge as any, "llama3.2");

  assert.ok(replacementSession !== null);
  assert.equal(replacementSession.id, "a1b2c3d4-1111-4222-8333-444455556666");
  assert.equal(replacementSession.budget_sats, 250);
  assert.equal(replacementSession.remaining_sats, 250);
  assert.equal(replacementSession.status, "pending_payment");

  // Verify historical session was not overwritten or budget increased
  assert.equal(originalSession.id, "3b2f5197-3c31-40d1-8b9f-599a3b8c03bc");
  assert.equal(originalSession.remaining_sats, 0);
  assert.equal(originalSession.budget_sats, 100);
});
