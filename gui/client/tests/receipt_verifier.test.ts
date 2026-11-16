import test from 'node:test';
import assert from 'node:assert/strict';
import {
  verifySignedReceipt,
  toCanonicalReceiptJson,
  toSignedBytes,
  RECEIPT_DOMAIN_PREFIX,
} from '../src/lib/crypto/receipt-verify';
import type { SignedReceipt } from '../src/lib/api/receipts';

// Genuine test vector generated and stored by the SatsGrid Rust backend node in .satsgrid/state.db
const RUST_SIGNED_RECEIPT: SignedReceipt = {
  payload: {
    receipt_id: 'rcpt_0770dab627e94197a5b4fb0f93197d54',
    schema_version: 1,
    key_id: 'ed25519:87257580f13e2ba9',
    provider_id: 'satsgrid-node',
    request_id: 'cdc09224-8684-4680-9be6-ed2727005d95',
    session_id: '3b2f5197-3c31-40d1-8b9f-599a3b8c03bc',
    model: 'llama3.2:latest',
    inference_status: 'completed',
    payment_status: 'settled',
    payment_hash: 'd1a648a0a13b42a03b22b4eb4d9e00a7ead4e696916003ff76d485451f6369e8',
    approved_max_sats: 10,
    actual_charge_sats: 10,
    currency: 'sats',
    prompt_tokens: 558,
    created_at: 1794733348,
  },
  signature_hex:
    '951a21f8355b22a9d9189297c58f54ca952e9c0289d090448d516c1a738808129c8051b77f79614969b5e3a10f7430e7d3c1ade3856e4151067cbc6ad4a2fe0b',
  public_key_hex:
    '87257580f13e2ba937fd5f7849cc8610311308cff0f8a6a551bf6ebaf92fa6e9',
  algorithm: 'ed25519',
  key_id: 'ed25519:87257580f13e2ba9',
};

// Test Vector 2 with completion_tokens included from Rust state.db
const RUST_SIGNED_RECEIPT_WITH_COMPLETION: SignedReceipt = {
  payload: {
    receipt_id: 'rcpt_a31146abb1fd42d19623500266bdec32',
    schema_version: 1,
    key_id: 'ed25519:87257580f13e2ba9',
    provider_id: 'satsgrid-node',
    request_id: '4f5ac268-456a-420a-8da9-2a077c6fd39e',
    session_id: '03e2f1df-6b9e-4c77-9240-b4a75c88291a',
    model: 'llama3.2:latest',
    inference_status: 'completed',
    payment_status: 'settled',
    payment_hash: 'd6202c4f230660a4f2d5ae26158a78d3fcf3b9595ae087db5e96d5b99229c8a5',
    approved_max_sats: 10,
    actual_charge_sats: 10,
    currency: 'sats',
    prompt_tokens: 36,
    completion_tokens: 8,
    created_at: 1794502894,
  },
  signature_hex:
    '4d2684d39dd95f70fa154514fe4cffde5f44bc7d364b965bcb3e10341222821080d03e0b86f5dffd0e9dc6969636adf31252356bfc27f8235be58a8c30057408',
  public_key_hex:
    '87257580f13e2ba937fd5f7849cc8610311308cff0f8a6a551bf6ebaf92fa6e9',
  algorithm: 'ed25519',
  key_id: 'ed25519:87257580f13e2ba9',
};

// 1. Valid signature, no expected key
test('Receipt Verifier - Case 1: Valid signature, no expected key', () => {
  const result = verifySignedReceipt(RUST_SIGNED_RECEIPT);
  assert.equal(result.status, 'valid');
  assert.equal(result.isSignatureValid, true);
  assert.equal(result.isKeyMatch, null);
  assert.equal(result.identityTrust, 'unverified');
  assert.ok(result.details.includes('unverified / self-asserted'));
});

// 2. Valid signature, matching expected key
test('Receipt Verifier - Case 2: Valid signature, matching expected key', () => {
  const trustedKey =
    '87257580f13e2ba937fd5f7849cc8610311308cff0f8a6a551bf6ebaf92fa6e9';
  const result = verifySignedReceipt(RUST_SIGNED_RECEIPT, trustedKey);
  assert.equal(result.status, 'valid');
  assert.equal(result.isSignatureValid, true);
  assert.equal(result.isKeyMatch, true);
  assert.equal(result.identityTrust, 'trusted');
  assert.ok(result.details.includes('matches the independently verified provider public key'));
});

// 3. Valid signature, mismatching expected key
test('Receipt Verifier - Case 3: Valid signature, mismatching expected key', () => {
  const differentExpectedKey =
    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
  const result = verifySignedReceipt(RUST_SIGNED_RECEIPT, differentExpectedKey);
  // Status reflects identity mismatch rejection
  assert.equal(result.status, 'invalid');
  // Cryptographic signature validity on the payload is strictly preserved
  assert.equal(result.isSignatureValid, true);
  assert.equal(result.isKeyMatch, false);
  assert.equal(result.identityTrust, 'mismatch');
  assert.ok(result.details.includes('does NOT match expected key'));
});

// 4. Invalid signature, matching expected key
test('Receipt Verifier - Case 4: Invalid signature, matching expected key', () => {
  const trustedKey =
    '87257580f13e2ba937fd5f7849cc8610311308cff0f8a6a551bf6ebaf92fa6e9';
  const tampered: SignedReceipt = {
    ...RUST_SIGNED_RECEIPT,
    payload: {
      ...RUST_SIGNED_RECEIPT.payload,
      actual_charge_sats: 1, // tampered amount
    },
  };
  const result = verifySignedReceipt(tampered, trustedKey);
  assert.equal(result.status, 'invalid');
  assert.equal(result.isSignatureValid, false);
  assert.equal(result.isKeyMatch, true);
  assert.equal(result.identityTrust, 'trusted'); // Key matches expected, but signature failed
});

// 5. Invalid signature, no expected key
test('Receipt Verifier - Case 5: Invalid signature, no expected key', () => {
  const tampered: SignedReceipt = {
    ...RUST_SIGNED_RECEIPT,
    payload: {
      ...RUST_SIGNED_RECEIPT.payload,
      model: 'tampered-model',
    },
  };
  const result = verifySignedReceipt(tampered);
  assert.equal(result.status, 'invalid');
  assert.equal(result.isSignatureValid, false);
  assert.equal(result.isKeyMatch, null);
  assert.equal(result.identityTrust, 'unverified');
});

// 6. Malformed receipt
test('Receipt Verifier - Case 6: Malformed receipt', () => {
  const res1 = verifySignedReceipt(null);
  assert.equal(res1.status, 'malformed');
  assert.equal(res1.isSignatureValid, false);

  const res2 = verifySignedReceipt({
    ...RUST_SIGNED_RECEIPT,
    payload: {
      ...RUST_SIGNED_RECEIPT.payload,
      receipt_id: 12345 as unknown as string, // invalid type
    },
  });
  assert.equal(res2.status, 'malformed');

  const res3 = verifySignedReceipt({
    ...RUST_SIGNED_RECEIPT,
    public_key_hex: 'invalid_hex',
  });
  assert.equal(res3.status, 'malformed');
});

// 7. Unsupported algorithm
test('Receipt Verifier - Case 7: Unsupported algorithm', () => {
  const unsupported = {
    ...RUST_SIGNED_RECEIPT,
    algorithm: 'secp256k1',
  };
  const result = verifySignedReceipt(unsupported);
  assert.equal(result.status, 'unsupported');
  assert.equal(result.isSignatureValid, false);
  assert.ok(result.details.includes('Unsupported signature algorithm'));
});

// 8. Unsupported schema version
test('Receipt Verifier - Case 8: Unsupported schema version', () => {
  const unsupported = {
    ...RUST_SIGNED_RECEIPT,
    payload: {
      ...RUST_SIGNED_RECEIPT.payload,
      schema_version: 2,
    },
  };
  const result = verifySignedReceipt(unsupported);
  assert.equal(result.status, 'unsupported');
  assert.equal(result.isSignatureValid, false);
});

// 9. Tampered receipt ID, request ID, payment status
test('Receipt Verifier - Case 9: Tampered individual fields reject', () => {
  const tamperedId: SignedReceipt = {
    ...RUST_SIGNED_RECEIPT,
    payload: { ...RUST_SIGNED_RECEIPT.payload, receipt_id: 'rcpt_forged' },
  };
  assert.equal(verifySignedReceipt(tamperedId).status, 'invalid');

  const tamperedStatus: SignedReceipt = {
    ...RUST_SIGNED_RECEIPT,
    payload: { ...RUST_SIGNED_RECEIPT.payload, payment_status: 'failed' },
  };
  assert.equal(verifySignedReceipt(tamperedStatus).status, 'invalid');
});

// 10. Canonical serialization and omission test
test('Receipt Verifier - Case 10: Canonical serialization and omission test', () => {
  const canonical = toCanonicalReceiptJson(RUST_SIGNED_RECEIPT.payload);
  const expectedCanonical =
    '{"actual_charge_sats":10,"approved_max_sats":10,"created_at":1794733348,"currency":"sats","inference_status":"completed","key_id":"ed25519:87257580f13e2ba9","model":"llama3.2:latest","payment_hash":"d1a648a0a13b42a03b22b4eb4d9e00a7ead4e696916003ff76d485451f6369e8","payment_status":"settled","prompt_tokens":558,"provider_id":"satsgrid-node","receipt_id":"rcpt_0770dab627e94197a5b4fb0f93197d54","request_id":"cdc09224-8684-4680-9be6-ed2727005d95","schema_version":1,"session_id":"3b2f5197-3c31-40d1-8b9f-599a3b8c03bc"}';
  assert.equal(canonical, expectedCanonical);

  const signedBytes = toSignedBytes(canonical);
  const domainLen = new TextEncoder().encode(RECEIPT_DOMAIN_PREFIX).length;
  assert.equal(
    new TextDecoder().decode(signedBytes.slice(0, domainLen)),
    RECEIPT_DOMAIN_PREFIX
  );
});

// 11. Second vector with completion tokens
test('Receipt Verifier - Case 11: Second Rust vector with completion_tokens verifies', () => {
  const result = verifySignedReceipt(RUST_SIGNED_RECEIPT_WITH_COMPLETION);
  assert.equal(result.status, 'valid');
  assert.equal(result.isSignatureValid, true);
  assert.equal(result.identityTrust, 'unverified');
});
