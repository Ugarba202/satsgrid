import assert from "node:assert/strict";
import test from "node:test";
import { getSession, SessionReadError } from "@/lib/api/sessions";

test("getSession uses the authenticated session read endpoint and returns persisted metadata", async () => {
  const originalFetch = globalThis.fetch;
  let url = "";
  let authorization = "";
  let cache: RequestCache | undefined;
  globalThis.fetch = (async (input, init) => {
    url = String(input);
    authorization = new Headers(init?.headers).get("Authorization") || "";
    cache = init?.cache;
    return new Response(JSON.stringify({
      session_id: "session-uuid",
      status: "active",
      total_budget_sats: 100,
      remaining_sats: 70,
      reserved_sats: 10,
      spent_sats: 20,
      requests_count: 5,
      created_at: 1,
      expires_at: 2,
      closed_at: null,
    }), { status: 200, headers: { "Content-Type": "application/json" } });
  }) as typeof fetch;
  try {
    const snapshot = await getSession("session-uuid", { macaroon: "private-macaroon", preimage: "private-preimage" });
    assert.equal(url, "/v1/session/session-uuid");
    assert.equal(authorization, "L402 private-macaroon:private-preimage");
    assert.equal(cache, "no-store");
    assert.equal(snapshot.remaining_sats, 70);
    assert.equal(snapshot.reserved_sats, 10);
    assert.equal(snapshot.spent_sats, 20);
    assert.equal(snapshot.requests_count, 5);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("getSession preserves backward compatibility when requests_count is omitted", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => new Response(JSON.stringify({
    session_id: "legacy-session-uuid",
    status: "active",
    total_budget_sats: 100,
    remaining_sats: 100,
    reserved_sats: 0,
    spent_sats: 0,
    created_at: 1,
    expires_at: 2,
    closed_at: null,
  }), { status: 200, headers: { "Content-Type": "application/json" } })) as typeof fetch;
  try {
    const snapshot = await getSession("legacy-session-uuid", { macaroon: "mac", preimage: "pre" });
    assert.equal(snapshot.requests_count, undefined);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("getSession preserves authorization status and redacts credentials in errors", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => new Response(
    JSON.stringify({ error: { message: "Rejected private-macaroon private-preimage" } }),
    { status: 401, headers: { "Content-Type": "application/json" } },
  )) as typeof fetch;
  try {
    await assert.rejects(
      () => getSession("session-uuid", { macaroon: "private-macaroon", preimage: "private-preimage" }),
      (error: unknown) => {
        assert.ok(error instanceof SessionReadError);
        assert.equal(error.status, 401);
        assert.doesNotMatch(error.message, /private-macaroon|private-preimage/);
        return true;
      },
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});
