import assert from "node:assert/strict";
import { test } from "node:test";
import { closeSession } from "@/lib/api/sessions";

test("closeSession sends correct payload and L402 credentials", async () => {
  let interceptedUrl = "";
  let interceptedMethod = "";
  let interceptedHeaders: Record<string, string> = {};
  let interceptedBody = "";

  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async (url: any, init: any) => {
    interceptedUrl = String(url);
    interceptedMethod = init?.method || "GET";
    interceptedHeaders = (init?.headers as Record<string, string>) || {};
    interceptedBody = String(init?.body || "");

    return new Response(
      JSON.stringify({
        session_id: "00000000-0000-0000-0000-000000000001",
        status: "closed",
        total_budget_sats: 500,
        remaining_sats: 0,
        reserved_sats: 0,
        spent_sats: 500,
        closed_at: 1700000000,
        reason: "user_closed",
      }),
      { status: 200, headers: { "Content-Type": "application/json" } }
    );
  }) as any;

  try {
    const result = await closeSession(
      { session_id: "00000000-0000-0000-0000-000000000001", reason: "user_closed" },
      { macaroon: "test-macaroon-base64", preimage: "test-preimage-hex" }
    );

    assert.equal(interceptedUrl, "/v1/session/close");
    assert.equal(interceptedMethod, "POST");
    assert.equal(
      interceptedHeaders["Authorization"],
      "L402 test-macaroon-base64:test-preimage-hex"
    );
    assert.equal(interceptedHeaders["Content-Type"], "application/json");

    const parsedBody = JSON.parse(interceptedBody);
    assert.equal(parsedBody.session_id, "00000000-0000-0000-0000-000000000001");
    assert.equal(parsedBody.reason, "user_closed");

    assert.equal(result.status, "closed");
    assert.equal(result.remaining_sats, 0);
    assert.equal(result.spent_sats, 500);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("closeSession sends admin Bearer token when provided", async () => {
  let interceptedHeaders: Record<string, string> = {};

  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async (_url: any, init: any) => {
    interceptedHeaders = (init?.headers as Record<string, string>) || {};
    return new Response(
      JSON.stringify({
        session_id: "admin-target-id",
        status: "closed",
        total_budget_sats: 1000,
        remaining_sats: 0,
        reserved_sats: 0,
        spent_sats: 1000,
      }),
      { status: 200, headers: { "Content-Type": "application/json" } }
    );
  }) as any;

  try {
    const result = await closeSession(
      { session_id: "admin-target-id", reason: "admin_cleanup" },
      undefined,
      "super-admin-secret"
    );

    assert.equal(
      interceptedHeaders["Authorization"],
      "Bearer super-admin-secret"
    );
    assert.equal(result.status, "closed");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("closeSession throws formatted error message on backend rejection", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => {
    return new Response(
      JSON.stringify({
        error: "Cannot close session: 50 sats currently reserved by active requests",
      }),
      { status: 500, headers: { "Content-Type": "application/json" } }
    );
  }) as any;

  try {
    await assert.rejects(
      async () => {
        await closeSession({ session_id: "active-res-session" });
      },
      {
        message: "Cannot close session: 50 sats currently reserved by active requests",
      }
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});
