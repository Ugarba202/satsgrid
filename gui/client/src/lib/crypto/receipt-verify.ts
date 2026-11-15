import { ed25519 } from '@noble/curves/ed25519.js';
import type { SignedReceipt, ReceiptPayload } from '@/lib/api/receipts';

export const RECEIPT_DOMAIN_PREFIX = 'satsgrid:receipt:v1:';
export const ED25519_ALGORITHM_NAME = 'ed25519';

/**
 * High-level verification outcome:
 * - valid: Cryptographic signature is valid for the payload and signing key.
 * - invalid: Cryptographic signature is invalid or payload has been tampered with.
 * - malformed: Receipt structure, field types, or hex encodings are malformed.
 * - unsupported: Algorithm or schema version is not supported.
 * - verification_error: Verification could not be completed.
 */
export type ReceiptVerificationStatus =
  | 'valid'
  | 'invalid'
  | 'malformed'
  | 'unsupported'
  | 'verification_error';

/**
 * Provider identity trust status:
 * - trusted: Signing public key matches an independently verified/trusted expected public key.
 * - mismatch: An expected key was supplied, but the receipt was signed by a different key.
 * - unverified: No trusted key was provided; provider identity is self-asserted only.
 */
export type ProviderIdentityTrust = 'trusted' | 'mismatch' | 'unverified';

export interface ReceiptVerificationResult {
  /** Overall verification status reflecting structural validity and cryptographic verification */
  status: ReceiptVerificationStatus;
  /** Cryptographic signature validity */
  isSignatureValid: boolean;
  /** Whether the expected key matches (null if no expected key was supplied) */
  isKeyMatch: boolean | null;
  /** Provider identity trust classification */
  identityTrust: ProviderIdentityTrust;
  /** Algorithm indicated on receipt */
  algorithm?: string;
  /** Receipt identifier */
  receiptId?: string;
  /** Request identifier */
  requestId?: string;
  /** Provider identifier string */
  providerId?: string;
  /** Key identifier */
  keyId?: string;
  /** Public key hex from receipt */
  publicKeyHex?: string;
  /** Expected public key hex (if supplied) */
  expectedPublicKeyHex?: string;
  /** Detailed human-readable explanation */
  details: string;
  /** Exact canonical JSON string constructed for signature verification */
  canonicalJson?: string;
}

/**
 * Deterministically sort object keys recursively (matching RFC 8785 subset used by SatsGrid Rust backend).
 * Objects have keys sorted lexicographically; Arrays preserve element order while sorting nested objects;
 * Primitives remain unchanged.
 */
export function sortJsonKeys(val: unknown): unknown {
  if (val === null || typeof val !== 'object') {
    return val;
  }

  if (Array.isArray(val)) {
    return val.map(sortJsonKeys);
  }

  const obj = val as Record<string, unknown>;
  const sortedKeys = Object.keys(obj).sort();
  const sortedObj: Record<string, unknown> = {};

  for (const k of sortedKeys) {
    const v = obj[k];
    // skip undefined values matching serde skip_serializing_if behavior
    if (v !== undefined) {
      sortedObj[k] = sortJsonKeys(v);
    }
  }

  return sortedObj;
}

/**
 * Normalize and produce the exact canonical JSON for a ReceiptPayload.
 * The Rust backend defines ReceiptPayload with serde(skip_serializing_if = "Option::is_none").
 * Thus, optional fields that are null or undefined must be omitted from canonical serialization.
 */
export function toCanonicalReceiptJson(payload: ReceiptPayload | Record<string, unknown>): string {
  const cleanObj: Record<string, unknown> = {};

  const requiredFields = [
    'receipt_id',
    'schema_version',
    'key_id',
    'provider_id',
    'request_id',
    'model',
    'inference_status',
    'payment_status',
    'approved_max_sats',
    'actual_charge_sats',
    'currency',
    'created_at',
  ] as const;

  const optionalFields = [
    'session_id',
    'payment_hash',
    'prompt_tokens',
    'completion_tokens',
    'error_code',
    'reconciliation_id',
    'supersedes_receipt_id',
  ] as const;

  const raw = payload as Record<string, unknown>;

  for (const field of requiredFields) {
    if (raw[field] !== undefined && raw[field] !== null) {
      cleanObj[field] = raw[field];
    }
  }

  for (const field of optionalFields) {
    const val = raw[field];
    if (val !== undefined && val !== null) {
      cleanObj[field] = val;
    }
  }

  const sorted = sortJsonKeys(cleanObj);
  return JSON.stringify(sorted);
}

/**
 * Reconstruct the exact byte stream signed by the Rust backend:
 * [RECEIPT_DOMAIN_PREFIX bytes] + [canonical JSON UTF-8 bytes]
 */
export function toSignedBytes(canonicalJson: string): Uint8Array {
  const encoder = new TextEncoder();
  const domainBytes = encoder.encode(RECEIPT_DOMAIN_PREFIX);
  const jsonBytes = encoder.encode(canonicalJson);

  const merged = new Uint8Array(domainBytes.length + jsonBytes.length);
  merged.set(domainBytes, 0);
  merged.set(jsonBytes, domainBytes.length);
  return merged;
}

/**
 * Helper to convert a hex string to Uint8Array safely, rejecting invalid hex characters or odd lengths.
 */
export function hexToBytes(hex: string): Uint8Array | null {
  if (typeof hex !== 'string') return null;
  const cleanHex = hex.trim();
  if (cleanHex.length % 2 !== 0 || !/^[0-9a-fA-F]+$/.test(cleanHex)) {
    return null;
  }

  const bytes = new Uint8Array(cleanHex.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    const byte = parseInt(cleanHex.substring(i * 2, i * 2 + 2), 16);
    if (Number.isNaN(byte)) return null;
    bytes[i] = byte;
  }
  return bytes;
}

/**
 * Helper to convert bytes to hex string.
 */
export function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, '0'))
    .join('');
}

/**
 * Independently and cryptographically verify an Ed25519-signed SatsGrid inference receipt.
 *
 * CRITICAL SECURITY PRINCIPLES:
 * 1. Signature Validity: Checks mathematical proof that the payload was signed by `receipt.public_key_hex`.
 * 2. Key Matching: Checks whether `receipt.public_key_hex` matches an externally supplied expected public key.
 * 3. Identity Trust: Distinguishes whether the provider key is independently trusted (via configured trust roots
 *    or pinned operator keys) vs self-asserted. Even if an announcement is self-signed on Nostr, that alone does
 *    not constitute independent identity trust without out-of-band verification.
 *
 * @param receipt The signed receipt object to verify.
 * @param expectedPublicKeyHex Optional provider public key hex expected by the caller.
 */
export function verifySignedReceipt(
  receipt: unknown,
  expectedPublicKeyHex?: string
): ReceiptVerificationResult {
  if (!receipt || typeof receipt !== 'object') {
    return {
      status: 'malformed',
      isSignatureValid: false,
      isKeyMatch: null,
      identityTrust: 'unverified',
      details: 'Receipt payload is not an object or is null',
    };
  }

  const r = receipt as Partial<SignedReceipt>;

  // 1. Structural checks
  if (!r.payload || typeof r.payload !== 'object') {
    return {
      status: 'malformed',
      isSignatureValid: false,
      isKeyMatch: null,
      identityTrust: 'unverified',
      details: 'Receipt is missing required payload object',
    };
  }

  const payload = r.payload as Partial<ReceiptPayload>;

  // Check required fields
  if (
    typeof payload.receipt_id !== 'string' ||
    typeof payload.key_id !== 'string' ||
    typeof payload.provider_id !== 'string' ||
    typeof payload.request_id !== 'string' ||
    typeof payload.model !== 'string' ||
    typeof payload.inference_status !== 'string' ||
    typeof payload.payment_status !== 'string' ||
    typeof payload.currency !== 'string' ||
    typeof payload.created_at !== 'number' ||
    typeof payload.approved_max_sats !== 'number' ||
    typeof payload.actual_charge_sats !== 'number'
  ) {
    return {
      status: 'malformed',
      isSignatureValid: false,
      isKeyMatch: null,
      identityTrust: 'unverified',
      details: 'Receipt payload missing or has invalid types for mandatory fields',
    };
  }

  // 2. Schema version check
  if (payload.schema_version !== 1) {
    return {
      status: 'unsupported',
      isSignatureValid: false,
      isKeyMatch: null,
      identityTrust: 'unverified',
      receiptId: payload.receipt_id,
      details: `Unsupported receipt schema version: ${payload.schema_version}. Only version 1 is supported.`,
    };
  }

  // 3. Algorithm check
  if (r.algorithm !== ED25519_ALGORITHM_NAME) {
    return {
      status: 'unsupported',
      isSignatureValid: false,
      isKeyMatch: null,
      identityTrust: 'unverified',
      receiptId: payload.receipt_id,
      algorithm: r.algorithm,
      details: `Unsupported signature algorithm: '${r.algorithm || 'undefined'}'. Only 'ed25519' is supported.`,
    };
  }

  // 4. Public key extraction and validation
  if (typeof r.public_key_hex !== 'string') {
    return {
      status: 'malformed',
      isSignatureValid: false,
      isKeyMatch: null,
      identityTrust: 'unverified',
      receiptId: payload.receipt_id,
      details: 'Receipt missing public_key_hex',
    };
  }

  const receiptPubkeyHex = r.public_key_hex.trim().toLowerCase();
  const pubkeyBytes = hexToBytes(receiptPubkeyHex);
  if (!pubkeyBytes || pubkeyBytes.length !== 32) {
    return {
      status: 'malformed',
      isSignatureValid: false,
      isKeyMatch: null,
      identityTrust: 'unverified',
      receiptId: payload.receipt_id,
      publicKeyHex: r.public_key_hex,
      details: `Invalid public key encoding: expected 32-byte hex (64 hex characters), got ${receiptPubkeyHex.length} hex chars`,
    };
  }

  // 5. Signature extraction and validation
  if (typeof r.signature_hex !== 'string') {
    return {
      status: 'malformed',
      isSignatureValid: false,
      isKeyMatch: null,
      identityTrust: 'unverified',
      receiptId: payload.receipt_id,
      details: 'Receipt missing signature_hex',
    };
  }

  const sigHex = r.signature_hex.trim().toLowerCase();
  const sigBytes = hexToBytes(sigHex);
  if (!sigBytes || sigBytes.length !== 64) {
    return {
      status: 'malformed',
      isSignatureValid: false,
      isKeyMatch: null,
      identityTrust: 'unverified',
      receiptId: payload.receipt_id,
      details: `Invalid signature encoding: expected 64-byte hex (128 hex characters), got ${sigHex.length} hex chars`,
    };
  }

  // 6. Expected public key comparison
  let isKeyMatch: boolean | null = null;
  let identityTrust: ProviderIdentityTrust = 'unverified';
  let cleanExpected: string | undefined;

  if (expectedPublicKeyHex) {
    cleanExpected = expectedPublicKeyHex.trim().toLowerCase();
    const expectedBytes = hexToBytes(cleanExpected);
    if (!expectedBytes || expectedBytes.length !== 32) {
      return {
        status: 'verification_error',
        isSignatureValid: false,
        isKeyMatch: false,
        identityTrust: 'unverified',
        receiptId: payload.receipt_id,
        expectedPublicKeyHex,
        details: 'Provided expected provider public key is not a valid 32-byte hex string',
      };
    }

    if (cleanExpected === receiptPubkeyHex) {
      isKeyMatch = true;
      identityTrust = 'trusted';
    } else {
      isKeyMatch = false;
      identityTrust = 'mismatch';
    }
  }

  // 7. Canonical serialization and signed message bytes reconstruction
  let canonicalJson: string;
  let signedBytes: Uint8Array;
  try {
    canonicalJson = toCanonicalReceiptJson(payload as ReceiptPayload);
    signedBytes = toSignedBytes(canonicalJson);
  } catch (err) {
    return {
      status: 'verification_error',
      isSignatureValid: false,
      isKeyMatch,
      identityTrust,
      receiptId: payload.receipt_id,
      details: `Failed to construct canonical signed bytes: ${err instanceof Error ? err.message : String(err)}`,
    };
  }

  // 8. Cryptographic verification of the signature
  try {
    const isSignatureValid = ed25519.verify(sigBytes, signedBytes, pubkeyBytes);

    // If key mismatch occurred, overall status is 'invalid' regarding the expected identity,
    // but we strictly preserve isSignatureValid so the caller knows if the math holds
    if (isKeyMatch === false) {
      return {
        status: 'invalid',
        isSignatureValid,
        isKeyMatch: false,
        identityTrust: 'mismatch',
        receiptId: payload.receipt_id,
        requestId: payload.request_id,
        providerId: payload.provider_id,
        keyId: r.key_id,
        publicKeyHex: receiptPubkeyHex,
        expectedPublicKeyHex: cleanExpected,
        canonicalJson,
        details: isSignatureValid
          ? `Signature is cryptographically valid for key ${receiptPubkeyHex}, but does NOT match expected key ${cleanExpected}`
          : `Signature is cryptographically invalid AND does not match expected key ${cleanExpected}`,
      };
    }

    if (!isSignatureValid) {
      return {
        status: 'invalid',
        isSignatureValid: false,
        isKeyMatch,
        identityTrust: isKeyMatch ? 'trusted' : 'unverified',
        receiptId: payload.receipt_id,
        requestId: payload.request_id,
        providerId: payload.provider_id,
        keyId: r.key_id,
        publicKeyHex: receiptPubkeyHex,
        expectedPublicKeyHex: cleanExpected,
        canonicalJson,
        details: 'Ed25519 cryptographic signature verification failed: signature does not match signed payload',
      };
    }

    // Valid signature
    return {
      status: 'valid',
      isSignatureValid: true,
      isKeyMatch,
      identityTrust,
      receiptId: payload.receipt_id,
      requestId: payload.request_id,
      providerId: payload.provider_id,
      keyId: r.key_id,
      publicKeyHex: receiptPubkeyHex,
      expectedPublicKeyHex: cleanExpected,
      canonicalJson,
      details:
        identityTrust === 'trusted'
          ? 'Cryptographic signature is valid and matches the independently verified provider public key'
          : 'Cryptographic signature is valid against the embedded provider public key (provider identity is unverified / self-asserted)',
    };
  } catch (err) {
    return {
      status: 'verification_error',
      isSignatureValid: false,
      isKeyMatch,
      identityTrust,
      receiptId: payload.receipt_id,
      details: `Ed25519 verification runtime error: ${err instanceof Error ? err.message : String(err)}`,
    };
  }
}
