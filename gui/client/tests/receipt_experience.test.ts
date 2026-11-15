import test from "node:test";
import assert from "node:assert/strict";
import { chatCompletion, streamChatCompletion } from "@/lib/api/inference";
import { getReceipt, type SignedReceipt } from "@/lib/api/receipts";

const request = { model: "llama3.2", messages: [{ role: "user" as const, content: "hello" }] };

test("non-streaming completion captures x-satsgrid-receipt-id header and calls onReceiptId", async () => {
  const originalFetch = globalThis.fetch;
  let capturedReceiptId: string | null = null;
  let refreshed = false;

  globalThis.fetch = (async () => new Response(
    JSON.stringify({ id: "chatcmpl-1", choices: [{ message: { role: "assistant", content: "hi" } }] }),
    {
      status: 200,
      headers: {
        "Content-Type": "application/json",
        "x-satsgrid-receipt-id": "rcpt_nonstream_12345",
      },
    }
  )) as typeof fetch;

  try {
    await chatCompletion(
      request,
      "test-macaroon",
      "test-preimage",
      () => { refreshed = true; },
      (receiptId) => { capturedReceiptId = receiptId; }
    );
    assert.equal(refreshed, true);
    assert.equal(capturedReceiptId, "rcpt_nonstream_12345");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("streaming completion captures event: receipt and delivers receipt_id before [DONE]", async () => {
  const originalFetch = globalThis.fetch;
  let capturedReceiptId: string | null = null;
  let completed = false;
  const chunks: string[] = [];

  const ssePayload = [
    "data: {\"choices\":[{\"delta\":{\"content\":\"Hello \"}}]}",
    "",
    "data: {\"choices\":[{\"delta\":{\"content\":\"world!\"}}]}",
    "",
    "event: receipt",
    "data: {\"choices\":[],\"receipt_id\":\"rcpt_stream_67890\"}",
    "",
    "data: [DONE]",
    "",
    "",
  ].join("\n");

  globalThis.fetch = (async () => new Response(ssePayload, {
    status: 200,
    headers: { "Content-Type": "text/event-stream" },
  })) as typeof fetch;

  try {
    await streamChatCompletion(
      request,
      "test-macaroon",
      "test-preimage",
      (chunk) => chunks.push(chunk),
      undefined,
      undefined,
      undefined,
      () => { completed = true; },
      (receiptId) => { capturedReceiptId = receiptId; }
    );

    assert.equal(completed, true);
    assert.deepEqual(chunks, ["Hello ", "world!"]);
    assert.equal(capturedReceiptId, "rcpt_stream_67890");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("streaming completion captures receipt_id in standard data chunk if emitted without event line", async () => {
  const originalFetch = globalThis.fetch;
  let capturedReceiptId: string | null = null;

  const ssePayload = [
    "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}",
    "",
    "data: {\"choices\":[],\"receipt_id\":\"rcpt_chunk_112233\"}",
    "",
    "data: [DONE]",
    "",
    "",
  ].join("\n");

  globalThis.fetch = (async () => new Response(ssePayload, {
    status: 200,
    headers: { "Content-Type": "text/event-stream" },
  })) as typeof fetch;

  try {
    await streamChatCompletion(
      request,
      "test-macaroon",
      "test-preimage",
      () => {},
      undefined,
      undefined,
      undefined,
      undefined,
      (receiptId) => { capturedReceiptId = receiptId; }
    );

    assert.equal(capturedReceiptId, "rcpt_chunk_112233");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("streaming completion without receipt does not invoke onReceiptId", async () => {
  const originalFetch = globalThis.fetch;
  let receiptInvoked = false;

  const ssePayload = [
    "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}",
    "",
    "data: [DONE]",
    "",
    "",
  ].join("\n");

  globalThis.fetch = (async () => new Response(ssePayload, {
    status: 200,
    headers: { "Content-Type": "text/event-stream" },
  })) as typeof fetch;

  try {
    await streamChatCompletion(
      request,
      "test-macaroon",
      "test-preimage",
      () => {},
      undefined,
      undefined,
      undefined,
      undefined,
      () => { receiptInvoked = true; }
    );

    assert.equal(receiptInvoked, false);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("getReceipt fetches from /v1/receipts/:id and returns parsed SignedReceipt", async () => {
  const originalFetch = globalThis.fetch;
  const mockReceipt: SignedReceipt = {
    payload: {
      receipt_id: "rcpt_test_999",
      schema_version: 1,
      key_id: "key-123",
      provider_id: "node-1",
      request_id: "req-abc",
      model: "llama3.2",
      inference_status: "completed",
      payment_status: "settled",
      approved_max_sats: 100,
      actual_charge_sats: 25,
      currency: "sats",
      prompt_tokens: 15,
      completion_tokens: 30,
      created_at: 1731620000,
    },
    signature_hex: "abcd1234deadbeef",
    public_key_hex: "0102030405060708",
    algorithm: "Ed25519",
    key_id: "key-123",
  };

  let fetchedUrl = "";
  globalThis.fetch = (async (url: string | URL | Request) => {
    fetchedUrl = String(url);
    return new Response(JSON.stringify(mockReceipt), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  }) as typeof fetch;

  try {
    const res = await getReceipt("rcpt_test_999");
    assert.equal(fetchedUrl, "/v1/receipts/rcpt_test_999");
    assert.deepEqual(res, mockReceipt);
    assert.equal(res.payload.actual_charge_sats, 25);
    assert.equal(res.algorithm, "Ed25519");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("getReceipt throws descriptive error when receipt is not found", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => new Response(
    JSON.stringify({ error: "Receipt not found: rcpt_missing" }),
    {
      status: 404,
      headers: { "Content-Type": "application/json" },
    }
  )) as typeof fetch;

  try {
    await assert.rejects(
      () => getReceipt("rcpt_missing"),
      /Receipt not found: rcpt_missing/
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});
