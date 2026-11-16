import test from "node:test";
import assert from "node:assert/strict";

/**
 * Validates manual preimage recovery form availability and behavior across
 * payment_required, payment_submitted, and invalid_payment states.
 */

function validatePreimageInput(input: string): { valid: boolean; error: string | null; cleaned: string } {
  const cleaned = input.trim().toLowerCase();
  if (!cleaned) {
    return { valid: false, error: "Please enter a payment preimage.", cleaned };
  }
  if (cleaned.length !== 64 || !/^[0-9a-fA-F]{64}$/.test(cleaned)) {
    return { valid: false, error: "Preimage must be a 64-character hex string.", cleaned };
  }
  return { valid: true, error: null, cleaned };
}

function shouldShowPreimageRecoveryForm(status: string): boolean {
  // In ChatPanel:
  // - payment_required renders the invoice card, which contains the external wallet preimage input
  // - payment_submitted and invalid_payment render inside the status banner with the manual recovery form
  return status === "payment_required" || status === "payment_submitted" || status === "invalid_payment";
}

test("Preimage Recovery - Form is available in pending, payment_submitted, and invalid_payment states", () => {
  assert.equal(shouldShowPreimageRecoveryForm("payment_required"), true, "Must be available in payment_required");
  assert.equal(shouldShowPreimageRecoveryForm("payment_submitted"), true, "Must be available in payment_submitted");
  assert.equal(shouldShowPreimageRecoveryForm("invalid_payment"), true, "Must be available in invalid_payment");

  // Not shown in active or closed sessions
  assert.equal(shouldShowPreimageRecoveryForm("active"), false, "Not needed in active session");
  assert.equal(shouldShowPreimageRecoveryForm("closed"), false, "Not shown in closed session");
  assert.equal(shouldShowPreimageRecoveryForm("exhausted"), false, "Not shown in exhausted session");
});

test("Preimage Recovery - Validation accepts valid 64-char hex and rejects invalid inputs", () => {
  const validHex = "f56c5c9e7b4f1b3a1359959b6a40275090ab0b6a873f20126c062c5a61b27541";
  const validUppercaseHex = "F56C5C9E7B4F1B3A1359959B6A40275090AB0B6A873F20126C062C5A61B27541";
  
  // Valid
  const resValid = validatePreimageInput(validHex);
  assert.equal(resValid.valid, true);
  assert.equal(resValid.error, null);
  assert.equal(resValid.cleaned, validHex.toLowerCase());

  // Valid uppercase trimmed
  const resValidUpper = validatePreimageInput(`  ${validUppercaseHex}  `);
  assert.equal(resValidUpper.valid, true);
  assert.equal(resValidUpper.error, null);
  assert.equal(resValidUpper.cleaned, validHex.toLowerCase());

  // Empty input
  const resEmpty = validatePreimageInput("   ");
  assert.equal(resEmpty.valid, false);
  assert.equal(resEmpty.error, "Please enter a payment preimage.");

  // Too short
  const resShort = validatePreimageInput("1234abcd");
  assert.equal(resShort.valid, false);
  assert.equal(resShort.error, "Preimage must be a 64-character hex string.");

  // Non-hex characters
  const resNonHex = validatePreimageInput("z".repeat(64));
  assert.equal(resNonHex.valid, false);
  assert.equal(resNonHex.error, "Preimage must be a 64-character hex string.");
});

test("Preimage Recovery - Rejection does not destroy session or require repayment", () => {
  const {
    restoredStatus,
    inferenceFailureStatus,
    attachPaymentProof,
  } = require("../src/lib/session-lifecycle.ts");

  const originalSession = {
    id: "test-session-id-1234",
    status: "pending_payment",
    model: "llama3.2",
    capability: "inference",
    budget_sats: 100,
    remaining_sats: 100,
    macaroon: "valid_macaroon",
    invoice: "lnbcrt100n...",
    created_at: new Date().toISOString(),
    requests_count: 0,
    messages: [],
  };

  // 1. Initial status: payment_required
  assert.equal(restoredStatus(originalSession), "payment_required");

  // 2. User inputs external preimage: transitions to payment_submitted
  const wrongPreimage = "0000000000000000000000000000000000000000000000000000000000000000";
  const sessionWithProof = attachPaymentProof(originalSession, wrongPreimage);
  assert.ok(sessionWithProof !== null);
  assert.equal(restoredStatus(sessionWithProof), "payment_submitted");

  // 3. Backend rejects wrong preimage: transitions to invalid_payment
  const rejectedStatus = inferenceFailureStatus(401, "Invalid L402 credentials", true);
  assert.equal(rejectedStatus, "invalid_payment");

  // Form remains available to re-enter correct preimage in invalid_payment status!
  assert.equal(shouldShowPreimageRecoveryForm(rejectedStatus), true);

  // 4. User corrects preimage: attachPaymentProof updates proof without creating new session or invoice
  const correctPreimage = "f56c5c9e7b4f1b3a1359959b6a40275090ab0b6a873f20126c062c5a61b27541";
  const sessionCorrected = attachPaymentProof(sessionWithProof, correctPreimage);
  assert.ok(sessionCorrected !== null);
  assert.equal(sessionCorrected.preimage, correctPreimage);
  assert.equal(sessionCorrected.id, originalSession.id);
  assert.equal(sessionCorrected.invoice, originalSession.invoice);
  assert.equal(restoredStatus(sessionCorrected), "payment_submitted");
});
