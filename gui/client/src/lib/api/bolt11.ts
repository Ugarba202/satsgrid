/**
 * Lightning BOLT11 Invoice Inspection & Parsing Utilities.
 *
 * Provides safe, dependency-free inspection of BOLT11 invoices,
 * extracting timestamp, expiry, and expiration status without external network calls.
 */

const BECH32_CHARSET = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";

export interface Bolt11ExpiryInfo {
  timestamp: number;
  expirySeconds: number;
  expiresAt: number;
  isExpired: boolean;
  secondsRemaining: number;
}

/**
 * Parses timestamp and expiry tag ('x' tag, tag=6) from a BOLT11 invoice.
 * If the invoice does not specify an expiry tag, BOLT11 specifies a default of 3600 seconds (1 hour).
 */
export function parseBolt11Expiry(
  invoice?: string | null,
  currentUnixTime = Math.floor(Date.now() / 1000)
): Bolt11ExpiryInfo | null {
  if (!invoice || typeof invoice !== "string") return null;
  const clean = invoice.trim().toLowerCase();
  const sepIdx = clean.lastIndexOf("1");
  if (sepIdx < 0) return null;

  const data = clean.slice(sepIdx + 1);
  // A valid BOLT11 invoice requires at least 7 timestamp words and 104 signature words (111 words minimum)
  if (data.length < 111) return null;

  const words: number[] = [];
  for (let i = 0; i < data.length; i++) {
    const idx = BECH32_CHARSET.indexOf(data[i]);
    if (idx === -1) return null;
    words.push(idx);
  }

  // 1. Decode 35-bit timestamp (7 5-bit words)
  let timestamp = 0;
  for (let i = 0; i < 7; i++) {
    timestamp = timestamp * 32 + words[i];
  }

  // 2. Decode tagged fields between timestamp (word 7) and signature (last 104 words)
  const tagWords = words.slice(7, words.length - 104);
  let expirySeconds = 3600; // BOLT11 default expiry is 1 hour
  let idx = 0;

  while (idx + 3 <= tagWords.length) {
    const tag = tagWords[idx];
    const dataLen = tagWords[idx + 1] * 32 + tagWords[idx + 2];
    idx += 3;

    if (idx + dataLen > tagWords.length) {
      break;
    }

    if (tag === 6) {
      // Tag 6 is 'x': expiry time in seconds
      let val = 0;
      for (let i = 0; i < dataLen; i++) {
        val = val * 32 + tagWords[idx + i];
      }
      expirySeconds = val;
    }

    idx += dataLen;
  }

  const expiresAt = timestamp + expirySeconds;
  const secondsRemaining = Math.max(0, expiresAt - currentUnixTime);
  const isExpired = currentUnixTime >= expiresAt;

  return {
    timestamp,
    expirySeconds,
    expiresAt,
    isExpired,
    secondsRemaining,
  };
}

/**
 * Format remaining seconds into a human-readable mm:ss or hh:mm:ss string.
 */
export function formatSecondsRemaining(seconds: number): string {
  if (seconds <= 0) return "Expired";
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = Math.floor(seconds % 60);

  if (h > 0) {
    return `${h}h ${m.toString().padStart(2, "0")}m`;
  }
  return `${m.toString().padStart(2, "0")}:${s.toString().padStart(2, "0")}`;
}
