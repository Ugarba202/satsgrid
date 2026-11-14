import test from "node:test";
import assert from "node:assert/strict";
import { chatCompletion, InferenceRequestError, streamChatCompletion } from "@/lib/api/inference";

const request = { model: "llama3.2", messages: [{ role: "user" as const, content: "hello" }] };

test("backend rejection preserves status and message without echoing credentials", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => new Response(
    JSON.stringify({ error: { message: "L402 rejected secret-macaroon secret-preimage" } }),
    { status: 401, headers: { "Content-Type": "application/json" } }
  )) as typeof fetch;
  let authorized = false;
  try {
    await assert.rejects(
      () => streamChatCompletion(request, "secret-macaroon", "secret-preimage", () => {}, undefined, undefined, () => { authorized = true; }),
      (error: unknown) => {
        assert.ok(error instanceof InferenceRequestError);
        assert.equal(error.status, 401);
        assert.match(error.message, /HTTP 401/);
        assert.doesNotMatch(error.message, /secret-macaroon|secret-preimage/);
        return true;
      }
    );
    assert.equal(authorized, false);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("same credentials can retry and backend success confirms authorization", async () => {
  const originalFetch = globalThis.fetch;
  const authorizations: string[] = [];
  let attempt = 0;
  globalThis.fetch = (async (_url, init) => {
    authorizations.push(new Headers(init?.headers).get("Authorization") || "");
    attempt += 1;
    if (attempt === 1) return new Response(JSON.stringify({ error: "temporary inference failure" }), { status: 502 });
    return new Response("data: [DONE]\n\n", {
      status: 200,
      headers: { "Content-Type": "text/event-stream", "x-satsgrid-remaining-budget-sats": "90" },
    });
  }) as typeof fetch;
  let authorizationsConfirmed = 0;
  let completions = 0;
  try {
    await assert.rejects(() => streamChatCompletion(request, "same-macaroon", "same-preimage", () => {}));
    await streamChatCompletion(request, "same-macaroon", "same-preimage", () => {}, undefined, undefined, () => { authorizationsConfirmed += 1; }, () => { completions += 1; });
    assert.equal(authorizations.length, 2);
    assert.equal(authorizations[0], "L402 same-macaroon:same-preimage");
    assert.equal(authorizations[1], authorizations[0]);
    assert.equal(authorizationsConfirmed, 1);
    assert.equal(completions, 1);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("stream balance header is applied only after [DONE] and interrupted streams fail", async () => {
  const originalFetch = globalThis.fetch;
  let remainingUpdates = 0;
  let completions = 0;
  globalThis.fetch = (async () => new Response("data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n", {
    status: 200,
    headers: { "Content-Type": "text/event-stream", "x-satsgrid-remaining-budget-sats": "80" },
  })) as typeof fetch;
  try {
    await assert.rejects(() => streamChatCompletion(
      request, "session-macaroon", "session-preimage", () => {}, undefined,
      () => { remainingUpdates += 1; }, undefined, () => { completions += 1; },
    ), /before successful completion/);
    assert.equal(remainingUpdates, 0);
    assert.equal(completions, 0);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("non-streaming completion invokes its refresh hook only after a successful response", async () => {
  const originalFetch = globalThis.fetch;
  let refreshes = 0;
  globalThis.fetch = (async () => new Response(JSON.stringify({ choices: [] }), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  })) as typeof fetch;
  try {
    await chatCompletion(request, "session-macaroon", "session-preimage", () => { refreshes += 1; });
    assert.equal(refreshes, 1);
  } finally {
    globalThis.fetch = originalFetch;
  }
});
