export type SessionStatus =
  | "idle"
  | "creating"
  | "payment_required"
  | "authorizing"
  | "payment_submitted"
  | "active"
  | "invalid_payment"
  | "inference_error"
  | "exhausted"
  | "expired"
  | "closed"
  | "error";

export interface SessionData {
  id?: string;
  budget_sats: number;
  remaining_sats?: number;
  reserved_sats?: number;
  spent_sats?: number;
  expires_at?: number;
  backend_status?: "pending_payment" | "active" | "exhausted" | "expired" | "closed";
  capability?: string;
  model?: string;
  invoice?: string;
  macaroon?: string;
  preimage?: string;
}
