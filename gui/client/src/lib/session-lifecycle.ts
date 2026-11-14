import type { L402Challenge } from "@/types/api";
import type { StoredSession } from "@/lib/storage/sessions";
import type { SessionStatus } from "@/types/session";

const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const INVOICE_PATTERN = /^ln(?:bc|bcrt|tb|sb)[0-9a-z]{20,}$/i;
const PREIMAGE_PATTERN = /^[0-9a-f]{64}$/i;

export type RecoveryCredentials = {
  sessionId: string;
  invoice: string;
  macaroon: string;
  preimage: string;
  budgetSats: number;
  capability: string;
};

export function validateSessionId(value: unknown): value is string {
  return typeof value === "string" && UUID_PATTERN.test(value);
}

export function validateInvoice(value: unknown): value is string {
  return typeof value === "string" && INVOICE_PATTERN.test(value.trim());
}

export function validateMacaroon(value: unknown): value is string {
  return typeof value === "string" && value.trim().length > 0;
}

export function validatePreimage(value: unknown): value is string {
  return typeof value === "string" && PREIMAGE_PATTERN.test(value.trim());
}

export function sessionFromChallenge(
  challenge: L402Challenge,
  model: string,
  createdAt = new Date().toISOString()
): StoredSession | null {
  const caveats = challenge.parsedCaveats ?? {};
  const sessionId = caveats.session ?? caveats.session_id;
  const budgetSats = Number(caveats.budget);
  const capability = caveats.capability;

  if (
    !validateSessionId(sessionId) ||
    !validateMacaroon(challenge.macaroon) ||
    !validateInvoice(challenge.invoice) ||
    !Number.isSafeInteger(budgetSats) || budgetSats <= 0 ||
    capability !== "inference"
  ) {
    return null;
  }

  return {
    id: sessionId,
    status: "pending_payment",
    model,
    capability,
    budget_sats: budgetSats,
    remaining_sats: budgetSats,
    macaroon: challenge.macaroon,
    invoice: challenge.invoice,
    created_at: createdAt,
    requests_count: 0,
    messages: [],
  };
}

export function validateRecoveryCredentials(
  credentials: RecoveryCredentials
): boolean {
  return (
    validateSessionId(credentials.sessionId) &&
    validateInvoice(credentials.invoice) &&
    validateMacaroon(credentials.macaroon) &&
    validatePreimage(credentials.preimage) &&
    Number.isSafeInteger(credentials.budgetSats) && credentials.budgetSats > 0 &&
    credentials.capability === "inference"
  );
}

export function recoverySession(
  credentials: RecoveryCredentials,
  model: string,
  createdAt = new Date().toISOString()
): StoredSession | null {
  if (!validateRecoveryCredentials(credentials)) return null;
  return {
    id: credentials.sessionId,
    status: "pending_payment",
    model,
    capability: credentials.capability,
    budget_sats: credentials.budgetSats,
    remaining_sats: credentials.budgetSats,
    macaroon: credentials.macaroon,
    preimage: credentials.preimage.trim().toLowerCase(),
    invoice: credentials.invoice.trim(),
    created_at: createdAt,
    requests_count: 0,
    messages: [],
  };
}

export function attachPaymentProof(
  session: StoredSession,
  preimage: string
): StoredSession | null {
  if (!validatePreimage(preimage) || !validateMacaroon(session.macaroon)) return null;
  return { ...session, status: "pending_payment", preimage: preimage.trim().toLowerCase() };
}

export function markBackendConfirmed(session: StoredSession): StoredSession | null {
  if (session.status !== "pending_payment" || !session.preimage) return null;
  return { ...session, status: "active" };
}

export function walletFailureStatus(): SessionStatus {
  return "payment_required";
}

export function inferenceFailureStatus(
  httpStatus: number | undefined,
  message: string,
  hasPaymentProof: boolean
): SessionStatus {
  if (/close/i.test(message)) return "closed";
  if (/expir/i.test(message)) return "expired";
  if (/exhaust|budget|insufficient/i.test(message)) return "exhausted";
  if (httpStatus === 401 || httpStatus === 403) return "invalid_payment";
  return hasPaymentProof ? "payment_submitted" : "inference_error";
}

export function developmentRecoveryEnabled(nodeEnv: string | undefined): boolean {
  return nodeEnv === "development";
}

export function restoredStatus(session: StoredSession):
  | "payment_required"
  | "payment_submitted"
  | "active"
  | "closed"
  | "exhausted"
  | "expired" {
  if (session.status === "pending_payment") {
    return session.preimage ? "payment_submitted" : "payment_required";
  }
  return session.status;
}
