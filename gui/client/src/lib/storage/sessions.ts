import { Message } from "@/types/inference";

export interface StoredSession {
  id: string;
  status: "pending_payment" | "active" | "exhausted" | "expired" | "closed";
  model: string;
  capability: string;
  budget_sats: number;
  remaining_sats: number;
  reserved_sats?: number;
  spent_sats?: number;
  expires_at?: number;
  macaroon: string;
  preimage?: string;
  invoice?: string;
  created_at: string;
  closed_at?: number;
  closed_reason?: string;
  requests_count: number;
  messages?: Message[];
}

const STORAGE_KEY = "satsgrid_sessions_v1";

export function getStoredSessions(): StoredSession[] {
  if (typeof window === "undefined") return [];
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed : [];
  } catch (e) {
    console.error("Failed to load stored sessions", e);
    return [];
  }
}

export function saveStoredSession(session: StoredSession): boolean {
  if (typeof window === "undefined") return false;
  try {
    const sessions = getStoredSessions();
    const existingIndex = sessions.findIndex((s) => s.id === session.id);
    if (existingIndex >= 0) {
      const existing = sessions[existingIndex];
      if (
        (existing.macaroon && session.macaroon && existing.macaroon !== session.macaroon) ||
        (existing.invoice && session.invoice && existing.invoice !== session.invoice)
      ) return false;
      sessions[existingIndex] = {
        ...existing,
        ...session,
        macaroon: existing.macaroon || session.macaroon,
        invoice: existing.invoice || session.invoice,
        messages: session.messages ?? existing.messages,
      };
    } else {
      sessions.unshift(session);
    }
    localStorage.setItem(STORAGE_KEY, JSON.stringify(sessions));
    return true;
  } catch {
    return false;
  }
}

/** Save a 402 challenge without replacing an already-funded local session. */
export function savePendingStoredSession(session: StoredSession): boolean {
  if (typeof window === "undefined") return false;
  try {
    const sessions = getStoredSessions();
    const existing = sessions.find((stored) => stored.id === session.id);
    if (existing && existing.status !== "pending_payment") return false;
    if (existing && (
      existing.macaroon !== session.macaroon ||
      existing.invoice !== session.invoice ||
      existing.budget_sats !== session.budget_sats
    )) return false;
    const pending = { ...session, status: "pending_payment" as const };
    if (existing) {
      Object.assign(existing, pending, {
        created_at: existing.created_at,
        messages: session.messages ?? existing.messages,
      });
    } else {
      sessions.unshift(pending);
    }
    localStorage.setItem(STORAGE_KEY, JSON.stringify(sessions));
    return true;
  } catch {
    console.error("Failed to save pending session");
    return false;
  }
}

export function updateStoredSessionBudget(
  id: string,
  remainingSats: number,
  incrementRequests = true
): void {
  if (typeof window === "undefined") return;
  try {
    const sessions = getStoredSessions();
    const session = sessions.find((s) => s.id === id);
    if (session) {
      session.remaining_sats = remainingSats;
      if (incrementRequests) {
        session.requests_count = (session.requests_count || 0) + 1;
      }
      if (remainingSats <= 0) {
        session.status = "exhausted";
      }
      localStorage.setItem(STORAGE_KEY, JSON.stringify(sessions));
    }
  } catch (e) {
    console.error("Failed to update session budget", e);
  }
}

export function markStoredSessionClosed(
  id: string,
  closedAt?: number,
  reason?: string
): void {
  if (typeof window === "undefined") return;
  try {
    const sessions = getStoredSessions();
    const session = sessions.find((s) => s.id === id);
    if (session) {
      session.status = "closed";
      session.remaining_sats = 0;
      session.closed_at = closedAt || Date.now();
      session.closed_reason = reason;
      localStorage.setItem(STORAGE_KEY, JSON.stringify(sessions));
    }
  } catch (e) {
    console.error("Failed to mark session closed", e);
  }
}

export function updateStoredSessionMessages(id: string, messages: Message[]): void {
  if (typeof window === "undefined") return;
  try {
    const sessions = getStoredSessions();
    const session = sessions.find((s) => s.id === id);
    if (session) {
      session.messages = messages;
      localStorage.setItem(STORAGE_KEY, JSON.stringify(sessions));
    }
  } catch (e) {
    console.error("Failed to update session messages", e);
  }
}

export function deleteStoredSession(id: string): void {
  if (typeof window === "undefined") return;
  try {
    const sessions = getStoredSessions().filter((s) => s.id !== id);
    localStorage.setItem(STORAGE_KEY, JSON.stringify(sessions));
    if (getActiveSessionId() === id) {
      clearActiveSessionId();
    }
  } catch (e) {
    console.error("Failed to delete session", e);
  }
}

const ACTIVE_SESSION_ID_KEY = "satsgrid_active_session_id";

export function setActiveSessionId(id: string): void {
  if (typeof window === "undefined") return;
  localStorage.setItem(ACTIVE_SESSION_ID_KEY, id);
}

export function getActiveSessionId(): string | null {
  if (typeof window === "undefined") return null;
  return localStorage.getItem(ACTIVE_SESSION_ID_KEY);
}

export function clearActiveSessionId(): void {
  if (typeof window === "undefined") return;
  localStorage.removeItem(ACTIVE_SESSION_ID_KEY);
}

export function getStoredSessionById(id: string): StoredSession | null {
  const sessions = getStoredSessions();
  return sessions.find((s) => s.id === id) || null;
}
