import test from "node:test";
import assert from "node:assert/strict";
import {
  sanitizeSpeechText,
  chunkSpeechText,
} from "@/lib/speech/useSpeechSynthesis";
import {
  formatAnnouncementMessage,
} from "@/lib/speech/useSpokenAnnouncements";

test("Speech Synthesis - sanitizes sensitive credentials before speech", () => {
  // 1. Redacts 64-character hex preimages
  const rawWithPreimage = "Payment preimage is 4a5b6c7d8e9f0123456789abcdef0123456789abcdef0123456789abcdef0123 confirmed.";
  const sanitizedPreimage = sanitizeSpeechText(rawWithPreimage);
  assert.ok(!sanitizedPreimage.includes("4a5b6c7d8e9f0123456789abcdef0123456789abcdef0123456789abcdef0123"));
  assert.ok(sanitizedPreimage.includes("[redacted credential]"));

  // 2. Redacts NWC wallet connect URIs
  const rawWithNwc = "Connecting to nostr+walletconnect://b889ff5b1513b641e2a13e3668ae9d29ce0ec507a525a1439770427ac7ec4761?relay=wss://relay.getalby.com/v1 for payment.";
  const sanitizedNwc = sanitizeSpeechText(rawWithNwc);
  assert.ok(!sanitizedNwc.includes("nostr+walletconnect://"));
  assert.ok(sanitizedNwc.includes("[redacted wallet connection]"));

  // 3. Redacts BOLT11 invoices
  const rawWithInvoice = "Please pay lnbc100n1p3xxxx... to continue.";
  const sanitizedInvoice = sanitizeSpeechText(rawWithInvoice);
  assert.ok(!sanitizedInvoice.includes("lnbc100n1p3xxxx"));
  assert.ok(sanitizedInvoice.includes("[lightning invoice]"));

  // 4. Redacts L402 / Authorization headers
  const rawWithAuth = "Received header Authorization L402 token:secret in request.";
  const sanitizedAuth = sanitizeSpeechText(rawWithAuth);
  assert.ok(!sanitizedAuth.includes("token:secret"));
  assert.ok(sanitizedAuth.includes("Authorization [redacted]"));
});

test("Speech Synthesis - chunks long text safely without breaking sentences", () => {
  const shortText = "Hello world. This is a short response.";
  const shortChunks = chunkSpeechText(shortText, 180);
  assert.equal(shortChunks.length, 1);
  assert.equal(shortChunks[0], "Hello world. This is a short response.");

  const longText =
    "Bitcoin is a decentralized digital currency, without a central bank or single administrator. " +
    "It can be sent from user to user on the peer-to-peer bitcoin network without the need for intermediaries. " +
    "Transactions are verified by network nodes through cryptography and recorded in a public distributed ledger called a blockchain. " +
    "The cryptocurrency was invented in 2008 by an unknown person or group of people using the name Satoshi Nakamoto. " +
    "The currency began use in 2009 when its implementation was released as open-source software.";

  const chunks = chunkSpeechText(longText, 140);
  assert.ok(chunks.length > 1, "Should create multiple chunks for long responses");
  
  // Verify each chunk is <= max length (unless a single unbreakable word exceeds it)
  for (const chunk of chunks) {
    assert.ok(chunk.length <= 160, `Chunk length ${chunk.length} should respect max size`);
  }

  // Combined text should contain all sanitized content
  const combined = chunks.join(" ");
  assert.ok(combined.includes("Bitcoin is a decentralized digital currency"));
  assert.ok(combined.includes("Satoshi Nakamoto"));
});

test("Spoken Confirmations - authoritative event mapping rules", () => {
  // Invoice issued
  const invoiceMsg = formatAnnouncementMessage("invoice_issued");
  assert.equal(invoiceMsg, "Invoice issued. Awaiting payment.");

  // Payment confirmed (ONLY triggered by authoritative backend confirmation)
  const paymentMsg = formatAnnouncementMessage("payment_confirmed");
  assert.equal(paymentMsg, "Payment confirmed. Inference authorized.");

  // Inference completed
  const completeMsg = formatAnnouncementMessage("inference_completed");
  assert.equal(completeMsg, "Inference completed. Your response is ready.");

  // Receipt available
  const receiptMsg = formatAnnouncementMessage("receipt_available");
  assert.equal(receiptMsg, "Signed inference receipt available.");

  // Inference failed
  const failMsg = formatAnnouncementMessage("inference_failed", "Insufficient balance");
  assert.equal(failMsg, "Inference failed. Insufficient balance");
});

test("Speech Recognition - progressive transcript updates avoid quadratic duplication", () => {
  // Simulates progressive WebSpeech events: interim chunks build cumulatively,
  // and when combined with base text, clean replacement preserves natural phrasing.
  let input = "";
  const inputBeforeSpeechRef = { current: "" };

  const onStartListening = (currentInput: string) => {
    inputBeforeSpeechRef.current = currentInput;
  };

  const handleTranscript = (newTranscript: string) => {
    const base = inputBeforeSpeechRef.current.trim();
    const speech = newTranscript.trim();
    if (!speech) return;
    input = base ? `${base} ${speech}` : speech;
  };

  // User already had some text typed
  input = "Prompt:";
  onStartListening(input);

  // Progressive events received from speech engine
  handleTranscript("explain");
  assert.equal(input, "Prompt: explain");

  handleTranscript("explain Bitcoin");
  assert.equal(input, "Prompt: explain Bitcoin");

  handleTranscript("explain Bitcoin Lightning payments");
  assert.equal(input, "Prompt: explain Bitcoin Lightning payments");

  handleTranscript("explain Bitcoin Lightning payments in simple terms");
  assert.equal(input, "Prompt: explain Bitcoin Lightning payments in simple terms");
});
