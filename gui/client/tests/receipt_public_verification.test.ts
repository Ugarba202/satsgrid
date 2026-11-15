import test from "node:test";
import assert from "node:assert/strict";
import { getReceipt, type SignedReceipt } from "@/lib/api/receipts";
import { verifySignedReceipt } from "@/lib/crypto/receipt-verify";

// Genuine test vector from node state.db
const RUST_SIGNED_RECEIPT: SignedReceipt = {
  payload: {
    receipt_id: "rcpt_0770dab627e94197a5b4fb0f93197d54",
    schema_version: 1,
    key_id: "ed25519:87257580f13e2ba9",
    provider_id: "satsgrid-node",
    request_id: "cdc09224-8684-4680-9be6-ed2727005d95",
    session_id: "3b2f5197-3c31-40d1-8b9f-599a3b8c03bc",
    model: "llama3.2:latest",
    inference_status: "completed",
    payment_status: "settled",
    payment_hash: "d1a648a0a13b42a03b22b4eb4d9e00a7ead4e696916003ff76d485451f6369e8",
    approved_max_sats: 10,
    actual_charge_sats: 10,
    currency: "sats",
    prompt_tokens: 558,
    created_at: 1794733348,
  },
  signature_hex:
    "951a21f8355b22a9d9189297c58f54ca952e9c0289d090448d516c1a738808129c8051b77f79614969b5e3a10f7430e7d3c1ade3856e4151067cbc6ad4a2fe0b",
  public_key_hex:
    "87257580f13e2ba937fd5f7849cc8610311308cff0f8a6a551bf6ebaf92fa6e9",
  algorithm: "ed25519",
  key_id: "ed25519:87257580f13e2ba9",
};

test("Public Receipt Flow - 1. Retrieve existing receipt and verify signature independently in browser engine", async () => {
  const originalFetch = globalThis.fetch;
  let requestedUrl = "";

  globalThis.fetch = (async (url: string | URL | Request) => {
    requestedUrl = String(url);
    return new Response(JSON.stringify(RUST_SIGNED_RECEIPT), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  }) as typeof fetch;

  try {
    const receipt = await getReceipt("rcpt_0770dab627e94197a5b4fb0f93197d54");
    assert.equal(requestedUrl, "/v1/receipts/rcpt_0770dab627e94197a5b4fb0f93197d54");
    assert.equal(receipt.payload.receipt_id, "rcpt_0770dab627e94197a5b4fb0f93197d54");

    // Execute independent verification engine
    const verification = verifySignedReceipt(receipt);
    assert.equal(verification.status, "valid");
    assert.equal(verification.isSignatureValid, true);
    assert.equal(verification.identityTrust, "unverified"); // self-asserted embedded key
    assert.equal(verification.receiptId, "rcpt_0770dab627e94197a5b4fb0f93197d54");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("Public Receipt Flow - 2. Verify with matching expected provider key confirms trust", () => {
  const trustedKey = "87257580f13e2ba937fd5f7849cc8610311308cff0f8a6a551bf6ebaf92fa6e9";
  const verification = verifySignedReceipt(RUST_SIGNED_RECEIPT, trustedKey);

  assert.equal(verification.status, "valid");
  assert.equal(verification.isSignatureValid, true);
  assert.equal(verification.isKeyMatch, true);
  assert.equal(verification.identityTrust, "trusted");
  assert.ok(verification.details.includes("matches the independently verified provider public key"));
});

test("Public Receipt Flow - 3. Verify with mismatching expected provider key reports mismatch while preserving signature validity", () => {
  const untrustedKey = "0000000000000000000000000000000000000000000000000000000000000000";
  const verification = verifySignedReceipt(RUST_SIGNED_RECEIPT, untrustedKey);

  assert.equal(verification.status, "invalid");
  assert.equal(verification.isSignatureValid, true); // math remains valid
  assert.equal(verification.isKeyMatch, false);
  assert.equal(verification.identityTrust, "mismatch");
  assert.ok(verification.details.includes("does NOT match expected key"));
});

test("Public Receipt Flow - 4. Tampered payload results in invalid signature state", () => {
  const tampered: SignedReceipt = {
    ...RUST_SIGNED_RECEIPT,
    payload: {
      ...RUST_SIGNED_RECEIPT.payload,
      actual_charge_sats: 50, // forged charge
    },
  };
  const verification = verifySignedReceipt(tampered);
  assert.equal(verification.status, "invalid");
  assert.equal(verification.isSignatureValid, false);
});

test("Public Receipt Flow - 5. Malformed receipt results in malformed state", () => {
  const malformed = {
    ...RUST_SIGNED_RECEIPT,
    payload: {
      ...RUST_SIGNED_RECEIPT.payload,
      created_at: "not-a-number" as unknown as number,
    },
  };
  const verification = verifySignedReceipt(malformed);
  assert.equal(verification.status, "malformed");
  assert.equal(verification.isSignatureValid, false);
});

test("Public Receipt Flow - 6. Non-existent receipt produces not-found rejection", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => new Response(
    JSON.stringify({ error: "Receipt 'rcpt_missing_404' not found" }),
    { status: 404, headers: { "Content-Type": "application/json" } }
  )) as typeof fetch;

  try {
    await assert.rejects(
      () => getReceipt("rcpt_missing_404"),
      /Receipt 'rcpt_missing_404' not found/
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("Public Receipt Flow - 7. Network/node outage produces network error", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => {
    throw new Error("Failed to fetch: Connection refused");
  }) as typeof fetch;

  try {
    await assert.rejects(
      () => getReceipt("rcpt_0770dab627e94197a5b4fb0f93197d54"),
      /Connection refused/
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});
