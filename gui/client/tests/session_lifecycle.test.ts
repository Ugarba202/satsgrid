import test from "node:test";
import assert from "node:assert/strict";
import type { StoredSession } from "@/lib/storage/sessions";
import { getStoredSessionById, savePendingStoredSession, saveStoredSession } from "@/lib/storage/sessions";
import {
  attachPaymentProof,
  developmentRecoveryEnabled,
  inferenceFailureStatus,
  markBackendConfirmed,
  recoverySession,
  restoredStatus,
  sessionFromChallenge,
  walletFailureStatus,
  type RecoveryCredentials,
} from "@/lib/session-lifecycle";

const ID = "11111111-1111-4111-8111-111111111111";
const INVOICE = `lnbcrt100u1${"a".repeat(100)}`;
const MACAROON = "opaque-test-macaroon";
const PREIMAGE = "ab".repeat(32);

function createStorage() {
  const values = new Map<string, string>();
  return {
    getItem(key: string) { return values.get(key) ?? null; },
    setItem(key: string, value: string) { values.set(key, value); },
    removeItem(key: string) { values.delete(key); },
  };
}

function withBrowser<T>(run: () => T): T {
  const oldWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  const oldStorage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  Object.defineProperty(globalThis, "window", { configurable: true, value: {} });
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: createStorage() });
  try {
    return run();
  } finally {
    if (oldWindow) Object.defineProperty(globalThis, "window", oldWindow);
    else Reflect.deleteProperty(globalThis, "window");
    if (oldStorage) Object.defineProperty(globalThis, "localStorage", oldStorage);
    else Reflect.deleteProperty(globalThis, "localStorage");
  }
}

function pendingSession(): StoredSession {
  return {
    id: ID,
    status: "pending_payment",
    model: "llama3.2",
    capability: "inference",
    budget_sats: 100,
    remaining_sats: 100,
    macaroon: MACAROON,
    invoice: INVOICE,
    created_at: "2026-11-12T10:00:00.000Z",
    requests_count: 0,
    messages: [],
  };
}

test("valid 402 challenge becomes a pending session and rejects missing fields", () => {
  const challenge = {
    macaroon: MACAROON,
    invoice: INVOICE,
    parsedCaveats: { session: ID, budget: "100", capability: "inference" },
  };
  const session = sessionFromChallenge(challenge, "llama3.2", "2026-11-12T10:00:00.000Z");
  assert.equal(session?.id, ID);
  assert.equal(session?.status, "pending_payment");
  assert.equal(session?.created_at, "2026-11-12T10:00:00.000Z");
  assert.equal(session?.remaining_sats, 100);
  assert.equal(sessionFromChallenge({ ...challenge, parsedCaveats: { budget: "100", capability: "inference" } }, "llama3.2"), null);
  assert.equal(sessionFromChallenge({ ...challenge, macaroon: "" }, "llama3.2"), null);
  assert.equal(sessionFromChallenge({ ...challenge, invoice: "" }, "llama3.2"), null);
});

test("pending challenge persistence survives reload and protects existing credentials", () => {
  withBrowser(() => {
    const pending = pendingSession();
    assert.equal(savePendingStoredSession(pending), true);
    // A second read models restoration after a page refresh from localStorage.
    assert.equal(getStoredSessionById(ID)?.status, "pending_payment");
    assert.equal(getStoredSessionById(ID)?.invoice, INVOICE);
    assert.equal(getStoredSessionById(ID)?.macaroon, MACAROON);
    assert.equal(savePendingStoredSession({ ...pending, macaroon: "different-macaroon" }), false);
    assert.equal(getStoredSessionById(ID)?.macaroon, MACAROON);
    assert.equal(saveStoredSession({ ...pending, status: "active", macaroon: "different-macaroon" }), false);
  });
});

test("wallet success retains proof as pending; backend response alone confirms active", () => {
  const withProof = attachPaymentProof(pendingSession(), PREIMAGE);
  assert.equal(withProof?.status, "pending_payment");
  assert.equal(withProof?.preimage, PREIMAGE);
  assert.equal(markBackendConfirmed(pendingSession()), null);
  assert.equal(markBackendConfirmed(withProof!)?.status, "active");
  assert.equal(attachPaymentProof(pendingSession(), "invalid"), null);
});

test("wallet cancellation keeps the invoice pending and can be retried", () => {
  assert.equal(walletFailureStatus(), "payment_required");
  withBrowser(() => {
    const pending = pendingSession();
    savePendingStoredSession(pending);
    assert.equal(getStoredSessionById(ID)?.status, "pending_payment");
    assert.equal(getStoredSessionById(ID)?.invoice, INVOICE);
  });
});

test("failed inference keeps paid credentials pending and a retry can confirm them", () => {
  const paid = attachPaymentProof(pendingSession(), PREIMAGE)!;
  assert.equal(inferenceFailureStatus(502, "Upstream inference error", true), "payment_submitted");
  assert.equal(paid.preimage, PREIMAGE);
  assert.equal(markBackendConfirmed(paid)?.status, "active");
});

test("recovery validates supplied credentials and production disables recovery", () => {
  const credentials: RecoveryCredentials = {
    sessionId: ID,
    invoice: INVOICE,
    macaroon: MACAROON,
    preimage: PREIMAGE,
    budgetSats: 100,
    capability: "inference",
  };
  const recovered = recoverySession(credentials, "llama3.2");
  assert.equal(recovered?.status, "pending_payment");
  assert.equal(recovered?.preimage, PREIMAGE);
  assert.equal(recoverySession({ ...credentials, sessionId: "" }, "llama3.2"), null);
  assert.equal(developmentRecoveryEnabled("development"), true);
  assert.equal(developmentRecoveryEnabled("production"), false);
  assert.equal(developmentRecoveryEnabled(undefined), false);
});

test("restoration preserves payment lifecycle and failure categories", () => {
  assert.equal(restoredStatus(pendingSession()), "payment_required");
  assert.equal(restoredStatus({ ...pendingSession(), preimage: PREIMAGE }), "payment_submitted");
  assert.equal(restoredStatus({ ...pendingSession(), status: "active" }), "active");
  assert.equal(inferenceFailureStatus(401, "L402 verification failed", true), "invalid_payment");
  assert.equal(inferenceFailureStatus(402, "Session budget exhausted", true), "exhausted");
  assert.equal(inferenceFailureStatus(401, "Session has expired", true), "expired");
});
