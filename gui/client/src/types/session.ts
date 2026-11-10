export type SessionStatus = "idle" | "creating" | "payment_required" | "authorizing" | "active" | "closed" | "error";

export interface SessionData {
  id?: string;
  budget_sats: number;
  remaining_sats?: number;
  capability?: string;
  model?: string;
  invoice?: string;
  macaroon?: string;
  preimage?: string;
}
