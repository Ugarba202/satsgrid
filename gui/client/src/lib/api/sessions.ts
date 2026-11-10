import { fetchWithL402 } from "./client";

export interface CloseSessionPayload {
  session_id?: string;
  reason?: string;
}

export interface CloseSessionResult {
  session_id: string;
  status: "closed" | "already_closed";
  total_budget_sats: number;
  remaining_sats: number;
  reserved_sats: number;
  spent_sats: number;
  closed_at?: number;
  reason?: string;
}

export async function createSession(budgetSats: number) {
  return fetchWithL402("/v1/session/new", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ budget_sats: budgetSats }),
  });
}

/**
 * Close an active or pending session on the backend.
 *
 * Supports both L402 credentials authorization (user closing their own session)
 * and admin token authorization.
 */
export async function closeSession(
  payload: CloseSessionPayload,
  credentials?: { macaroon: string; preimage?: string },
  adminToken?: string
): Promise<CloseSessionResult> {
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
  };

  if (credentials?.macaroon && credentials?.preimage) {
    headers["Authorization"] = `L402 ${credentials.macaroon}:${credentials.preimage}`;
  } else if (adminToken) {
    headers["Authorization"] = `Bearer ${adminToken}`;
  }

  const res = await fetch("/v1/session/close", {
    method: "POST",
    headers,
    body: JSON.stringify(payload),
  });

  if (!res.ok) {
    const errorBody = await res.json().catch(() => ({}));
    throw new Error(
      errorBody.error?.message ||
        errorBody.error ||
        `Session closure failed with HTTP ${res.status}`
    );
  }

  return res.json();
}

