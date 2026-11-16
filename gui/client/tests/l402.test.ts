import test from "node:test";
import assert from "node:assert/strict";
import { parseL402Header, parseMacaroonCaveats } from "../src/lib/api/client";
import {
  getInvoiceNetworkInfo,
  formatNetworkDisplay,
  resolveNetworkIdentity,
} from "../src/lib/api/network";

test("L402 parser - parses standard quoted challenge header", () => {
  const header = 'L402 token="mock_macaroon_123", invoice="lnbcrt100u1mockhash"';
  const parsed = parseL402Header(header);

  assert.ok(parsed);
  assert.equal(parsed.macaroon, "mock_macaroon_123");
  assert.equal(parsed.invoice, "lnbcrt100u1mockhash");
});

test("L402 parser - parses standard LSAT compatibility header", () => {
  const header = 'LSAT macaroon="legacy_token_456", invoice="lnbc100n1mockinv"';
  const parsed = parseL402Header(header);

  assert.ok(parsed);
  assert.equal(parsed.macaroon, "legacy_token_456");
  assert.equal(parsed.invoice, "lnbc100n1mockinv");
});

test("L402 parser - parses unquoted token and invoice values", () => {
  const header = "L402 token=unquoted_macaroon_abc, invoice=lnbc500u1rawinvoice";
  const parsed = parseL402Header(header);

  assert.ok(parsed);
  assert.equal(parsed.macaroon, "unquoted_macaroon_abc");
  assert.equal(parsed.invoice, "lnbc500u1rawinvoice");
});

test("L402 parser - parses reversed parameter order", () => {
  const header = 'L402 invoice="lnbcrt50u1inv", token="tok999"';
  const parsed = parseL402Header(header);

  assert.ok(parsed);
  assert.equal(parsed.macaroon, "tok999");
  assert.equal(parsed.invoice, "lnbcrt50u1inv");
});

test("L402 parser - handles single-quoted values and extra whitespace", () => {
  const header = "   L402    invoice='lnbcrt10u',   token='single_quoted_token'   ";
  const parsed = parseL402Header(header);

  assert.ok(parsed);
  assert.equal(parsed.macaroon, "single_quoted_token");
  assert.equal(parsed.invoice, "lnbcrt10u");
});

test("L402 parser - handles additional parameters gracefully", () => {
  const header = 'L402 realm="satsgrid", token="my_macaroon", version="1", invoice="lnbcrt100"';
  const parsed = parseL402Header(header);

  assert.ok(parsed);
  assert.equal(parsed.macaroon, "my_macaroon");
  assert.equal(parsed.invoice, "lnbcrt100");
});

test("L402 parser - returns null for malformed or missing headers", () => {
  assert.equal(parseL402Header(null), null);
  assert.equal(parseL402Header(undefined), null);
  assert.equal(parseL402Header(""), null);
  assert.equal(parseL402Header('Bearer token="abc"'), null);
  assert.equal(parseL402Header('L402 token="only_token"'), null);
  assert.equal(parseL402Header('L402 invoice="only_invoice"'), null);
  assert.equal(parseL402Header('Basic realm="realm"'), null);
});

test("L402 parser - parses caveats from base64 macaroon", () => {
  const macaroonObj = {
    location: "satsgrid-node",
    identifier: "sess_123",
    caveats: ["session_id = sess_123", "budget = 1000"],
  };
  const b64 = Buffer.from(JSON.stringify(macaroonObj)).toString("base64");
  const caveats = parseMacaroonCaveats(b64);

  assert.equal(caveats["session_id"], "sess_123");
  assert.equal(caveats["budget"], "1000");
});

test("Network identifier - classifies mock invoices correctly", () => {
  const mockInv = "lnbc100mockabcdef123456";
  const info = getInvoiceNetworkInfo(mockInv);

  assert.equal(info.network, "mock");
  assert.equal(info.displayName, "Mock Lightning");
});

test("Network identifier - classifies regtest invoices correctly", () => {
  const regtestInv = "lnbcrt100u1pn25870...";
  const info = getInvoiceNetworkInfo(regtestInv);

  assert.equal(info.network, "regtest");
  assert.equal(info.displayName, "Bitcoin Regtest (Polar)");
});

test("Network identifier - classifies testnet invoices correctly", () => {
  const testnetInv = "lntb100u1pn25870...";
  const info = getInvoiceNetworkInfo(testnetInv);

  assert.equal(info.network, "testnet");
  assert.equal(info.displayName, "Bitcoin Testnet");

  const signetInv = "lnsbs100u1pn25870...";
  const signetInfo = getInvoiceNetworkInfo(signetInv);
  assert.equal(signetInfo.network, "signet");
  assert.equal(signetInfo.displayName, "Bitcoin Signet");
});

test("Network identifier - classifies real mainnet invoices correctly", () => {
  const mainnetInv = "lnbc100u1pn25870realpayment...";
  const info = getInvoiceNetworkInfo(mainnetInv);

  assert.equal(info.network, "mainnet");
  assert.equal(info.displayName, "Bitcoin Mainnet");
});

test("Network identifier - handles empty and unknown invoices", () => {
  assert.equal(getInvoiceNetworkInfo("").displayName, "Lightning Network");
  assert.equal(getInvoiceNetworkInfo(null).displayName, "Lightning Network");
  assert.equal(getInvoiceNetworkInfo("invalid_prefix").displayName, "Lightning Network");
});

test("Authoritative network identifier - formats backend and network combinations", () => {
  assert.equal(
    formatNetworkDisplay("mock", "testnet"),
    "Mock Lightning (testnet)"
  );
  assert.equal(
    formatNetworkDisplay("lnd_rest", "mainnet"),
    "Bitcoin Mainnet [LND]"
  );
  assert.equal(
    formatNetworkDisplay("nwc", "regtest"),
    "Bitcoin Regtest (Polar) [NWC]"
  );
  assert.equal(
    formatNetworkDisplay("lnd_rest", "signet"),
    "Bitcoin Signet [LND]"
  );
});

test("Authoritative network resolver - prefers node metadata over invoice heuristics", () => {
  // Even if an invoice string starts with "lnbc", authoritative node metadata overrides it
  const nodeLightning = { backend: "mock", network: "testnet" };
  const mockInvoice = "lnbc100mockabcdef";

  const resolved = resolveNetworkIdentity(nodeLightning, mockInvoice);
  assert.equal(resolved.isAuthoritative, true);
  assert.equal(resolved.backend, "mock");
  assert.equal(resolved.network, "testnet");
  assert.equal(resolved.displayName, "Mock Lightning (testnet)");
});

test("Authoritative network resolver - falls back gracefully when node is unavailable", () => {
  // When node is null (loading or unavailable), falls back safely to invoice-based inspection
  const resolved = resolveNetworkIdentity(null, "lnbcrt100u1pn25870");
  assert.equal(resolved.isAuthoritative, false);
  assert.equal(resolved.backend, "unknown");
  assert.equal(resolved.network, "regtest");
  assert.equal(resolved.displayName, "Bitcoin Regtest (Polar)");

  // When node is null and invoice is unknown
  const emptyResolved = resolveNetworkIdentity(null, null);
  assert.equal(emptyResolved.isAuthoritative, false);
  assert.equal(emptyResolved.displayName, "Lightning Network");
});

test("Authoritative network resolver - handles incomplete node metadata", () => {
  // Incomplete metadata should fall back to invoice inspection safely
  const incomplete = { backend: "", network: "" };
  const fallback = resolveNetworkIdentity(incomplete, "lnbc100u1pn25870realpayment");
  assert.equal(fallback.isAuthoritative, false);
  assert.equal(fallback.displayName, "Bitcoin Mainnet");
});
