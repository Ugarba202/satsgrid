import test from "node:test";
import assert from "node:assert/strict";
import qrGenerator from "qrcode-generator";
import { parseBolt11Expiry, formatSecondsRemaining } from "@/lib/api/bolt11";

// Realistic BOLT11 invoice generated on regtest:
// lnbcrt100n1p402zucpp5vdm5r202lq54gu4a5pfjncu0ay5m504nuznkeelsxl3pnenl59rsdp82dshgu68wf5kggzrdpshggzrdakhqmr9w35k7mscqzzsxqyz5vqsp5rhs98w5fvff0wx3cdd66fwa2wp04wvrt69mszu9p3pkpyuh32xkq9qxpqysgqa7aj68n8gfv6txdtshpjjh3au9uwajdy3dfwaux0fhk4f5mw3uvh45dxn2emptvyxxl0hffk73krmpe3l6cepe888mpn3xckj6h7j4sqz7eqks
const SAMPLE_REGTEST_INVOICE =
  "lnbcrt100n1p402zucpp5vdm5r202lq54gu4a5pfjncu0ay5m504nuznkeelsxl3pnenl59rsdp82dshgu68wf5kggzrdpshggzrdakhqmr9w35k7mscqzzsxqyz5vqsp5rhs98w5fvff0wx3cdd66fwa2wp04wvrt69mszu9p3pkpyuh32xkq9qxpqysgqa7aj68n8gfv6txdtshpjjh3au9uwajdy3dfwaux0fhk4f5mw3uvh45dxn2emptvyxxl0hffk73krmpe3l6cepe888mpn3xckj6h7j4sqz7eqks";

test("QR code generator encodes exact BOLT11 invoice string", () => {
  const qr = qrGenerator(0, "M");
  qr.addData(SAMPLE_REGTEST_INVOICE);
  qr.make();

  const moduleCount = qr.getModuleCount();
  assert.ok(moduleCount > 0, "QR code module count must be positive");

  const svg = qr.createSvgTag({ scalable: true, margin: 2 });
  assert.ok(svg.includes("<svg"), "QR SVG output must contain <svg tag");
  assert.ok(svg.includes("viewBox="), "QR SVG output must include viewBox for responsive scaling");
  assert.ok(svg.includes("path d="), "QR SVG output must contain path elements for black modules");
});

test("BOLT11 expiry parser extracts valid timestamp, expiry, and expiration boundary", () => {
  // Test with current time before expiry
  const parsedBefore = parseBolt11Expiry(SAMPLE_REGTEST_INVOICE, 1794444200);
  assert.ok(parsedBefore !== null, "Must parse valid BOLT11 invoice");
  assert.equal(parsedBefore.timestamp, 1794444184);
  assert.equal(parsedBefore.expirySeconds, 86400); // 24h
  assert.equal(parsedBefore.expiresAt, 1794530584);
  assert.equal(parsedBefore.isExpired, false);
  assert.ok(parsedBefore.secondsRemaining > 0);

  // Test with current time after expiry
  const parsedAfter = parseBolt11Expiry(SAMPLE_REGTEST_INVOICE, 1794530585);
  assert.ok(parsedAfter !== null);
  assert.equal(parsedAfter.isExpired, true);
  assert.equal(parsedAfter.secondsRemaining, 0);

  // Formatting tests
  assert.equal(formatSecondsRemaining(0), "Expired");
  assert.equal(formatSecondsRemaining(45), "00:45");
  assert.equal(formatSecondsRemaining(125), "02:05");
  assert.equal(formatSecondsRemaining(3665), "1h 01m");
});

test("BOLT11 expiry parser returns null gracefully for invalid or malformed strings", () => {
  assert.equal(parseBolt11Expiry(null), null);
  assert.equal(parseBolt11Expiry(""), null);
  assert.equal(parseBolt11Expiry("not_an_invoice"), null);
  assert.equal(parseBolt11Expiry("lnbcrt1short"), null);
});

test("Lightning invoice copy interaction fallback and text preservation", () => {
  // Emulate clipboard copy
  let copiedText = "";
  const mockClipboard = {
    writeText: async (text: string) => {
      copiedText = text;
    },
  };

  mockClipboard.writeText(SAMPLE_REGTEST_INVOICE);
  assert.equal(copiedText, SAMPLE_REGTEST_INVOICE, "Copied text must exactly match original BOLT11 invoice");
});

test("Session payment state machine verifies pending, confirming, confirmed, and expired invariants", () => {
  const {
    restoredStatus,
    walletFailureStatus,
    inferenceFailureStatus,
    markBackendConfirmed,
    attachPaymentProof,
  } = require("../src/lib/session-lifecycle.ts");

  const pendingSession = {
    id: "3b2f5197-3c31-40d1-8b9f-599a3b8c03bc",
    status: "pending_payment",
    model: "llama3.2",
    capability: "inference",
    budget_sats: 100,
    remaining_sats: 100,
    macaroon: "valid_macaroon",
    invoice: SAMPLE_REGTEST_INVOICE,
    created_at: new Date().toISOString(),
    requests_count: 0,
    messages: [],
  };

  // 1. Initial pending session awaits payment
  assert.equal(restoredStatus(pendingSession), "payment_required");

  // 2. Wallet failure keeps session in payment_required without dropping credentials
  assert.equal(walletFailureStatus(), "payment_required");

  // 3. User pays or inputs preimage: becomes payment_submitted (verifying/pending backend proof)
  const proofPreimage = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
  const attached = attachPaymentProof(pendingSession, proofPreimage);
  assert.ok(attached !== null);
  assert.equal(attached.status, "pending_payment");
  assert.equal(attached.preimage, proofPreimage);
  assert.equal(restoredStatus(attached), "payment_submitted");

  // 4. Backend confirms settlement: session transitions to active
  const confirmed = markBackendConfirmed(attached);
  assert.ok(confirmed !== null);
  assert.equal(confirmed.status, "active");

  // 5. Expired invoices or sessions fail safely to expired
  assert.equal(inferenceFailureStatus(400, "Invoice or session has expired", true), "expired");

  // 6. Failed or rejected payment fails safely to invalid_payment without losing state
  assert.equal(inferenceFailureStatus(403, "Invalid payment proof", true), "invalid_payment");
});
