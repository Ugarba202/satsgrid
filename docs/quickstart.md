# SatsGrid Developer Quickstart & Integration Guide

Welcome to the **SatsGrid Developer API Quickstart**. SatsGrid provides OpenAI-compatible generative AI inference endpoints with **zero accounts, zero API keys, and zero credit card billing**. 

Inference requests are authorized and metered strictly via **L402 (Lightning Service Authentication Tokens)** over the Bitcoin Lightning Network.

---

## Table of Contents
1. [Overview & Core Architecture](#1-overview--core-architecture)
2. [Prerequisites](#2-prerequisites)
3. [How L402 Authorization Works](#3-how-l402-authorization-works)
4. [API Endpoints Reference](#4-api-endpoints-reference)
5. [Inference Flow: Pay-Per-Request vs. Session Budgets](#5-inference-flow-pay-per-request-vs-session-budgets)
6. [Step-by-Step Implementation with cURL](#6-step-by-step-implementation-with-curl)
7. [Python Integration Example (Non-Streaming & Streaming)](#7-python-integration-example)
8. [Node.js / TypeScript Integration Example](#8-nodejs--typescript-integration-example)
9. [Response Headers & Cryptographic Receipts](#9-response-headers--cryptographic-receipts)
10. [Error Handling & Edge Cases](#10-error-handling--edge-cases)
11. [Security Considerations](#11-security-considerations)
12. [Polar Regtest vs. Mainnet Environments](#12-polar-regtest-vs-mainnet-environments)

---

## 1. Overview & Core Architecture

SatsGrid acts as an intelligent economic reverse proxy sitting in front of local or remote LLM runtimes (e.g. Ollama, vLLM, or upstream OpenAI-compatible servers):
- **Base URL**: `http://127.0.0.1:8080` (default local node)
- **Protocol**: HTTP/1.1 or HTTP/2 with JSON payloads and Server-Sent Events (SSE) for streaming.
- **Specification Compliance**: Fully OpenAI-compatible schema (`/v1/models`, `/v1/chat/completions`).
- **Payment Layer**: Lightning Network BOLT11 invoices generated on-demand by the node's Lightning backend (LND REST, Nostr Wallet Connect NIP-47, or Mock backend).

---

## 2. Prerequisites

To make inference requests against a SatsGrid node, you need:
1. **Network Access**: HTTP access to a running SatsGrid node (e.g., `http://127.0.0.1:8080`).
2. **Lightning Wallet**: Any BOLT11-capable Lightning wallet to settle invoices and retrieve payment preimages:
   - **Local Development / Regtest**: Polar LND node, `lncli`, or SatsGrid Mock Backend.
   - **Production / Signet / Mainnet**: Alby, Phoenix, Zeus, Mutiny, Breez, or any NWC-enabled wallet.
3. **HTTP Client**: `curl`, Python (`httpx` or `requests`), or Node.js (`fetch`).

---

## 3. How L402 Authorization Works

L402 couples two components:
1. **Macaroon (`token`)**: A cryptographically signed token issued by the SatsGrid node containing caveats (e.g. `session_id`, `budget`, `capability = inference`, `expires_at`).
2. **Preimage (`preimage`)**: A 32-byte secret (64 hex characters) revealed by the Lightning network exclusively upon invoice payment, where `SHA256(preimage) = payment_hash`.

### The 402 Handshake

```
Client                                                  SatsGrid Node
  |                                                           |
  |--- 1. POST /v1/chat/completions (Unauthenticated) ------->|
  |                                                           |
  |<-- 2. HTTP 402 Payment Required --------------------------|
  |       WWW-Authenticate: L402 token="...", invoice="..."   |
  |                                                           |
  |==== [Client pays BOLT11 invoice via Lightning Wallet] ====|
  |==== [Wallet returns 32-byte payment preimage] ============|
  |                                                           |
  |--- 3. POST /v1/chat/completions ------------------------->|
  |       Authorization: L402 <token>:<preimage>              |
  |                                                           |
  |<-- 4. HTTP 200 OK (Inference Response + Receipt Header)---|
```

---

## 4. API Endpoints Reference

| Method | Endpoint | Auth Required | Purpose |
| :--- | :--- | :--- | :--- |
| `GET` | `/health` | None (Public) | Check node status, lightning backend, and bitcoin network |
| `GET` | `/v1/models` | None (Public) | List available models and pricing |
| `POST` | `/v1/session/new` | None (Returns 402) | Initialize a multi-request session budget |
| `GET` | `/v1/session/:id` | L402 (Session) | Query real-time balance and usage for an active session |
| `POST` | `/v1/session/close`| L402 or Admin | Voluntarily close a session and finalize accounting |
| `POST` | `/v1/chat/completions` | L402 | Perform AI chat completion (streaming or non-streaming) |
| `GET` | `/v1/receipts/:id` | None (Public) | Retrieve signed Ed25519 cryptographic inference receipt |

---

## 5. Inference Flow: Pay-Per-Request vs. Session Budgets

SatsGrid supports two distinct payment paradigms:

### A. Session Budgets (Recommended for Agents and Interactive Chats)
Pre-fund a balance (e.g. 100 or 500 sats) with a single Lightning payment. All subsequent inference requests debit from this session balance until exhausted, eliminating per-prompt payment latency.

1. Client calls `POST /v1/session/new` with `{"budget_sats": 200}`.
2. Node returns `402 Payment Required` with a 200-sat invoice and a session-bound macaroon.
3. Client pays invoice once, acquires preimage.
4. Client executes multiple prompt turns with `Authorization: L402 <token>:<preimage>`.
5. Node lazily activates session on first inference request and debits flat rate or per-token sats.

### B. Single Pay-Per-Request (PPR)
Ideal for one-off tasks:
1. Client calls `POST /v1/chat/completions` directly without an `Authorization` header.
2. Node returns `402 Payment Required` with an invoice matching the exact single-request flat price.
3. Client pays invoice and resubmits with `Authorization: L402 <token>:<preimage>`.

---

## 6. Step-by-Step Implementation with cURL

### Step 1: Check Node Health & Available Models
```bash
# Verify the node is online and discover the network
curl -s http://127.0.0.1:8080/health | jq .

# List available models
curl -s http://127.0.0.1:8080/v1/models | jq .
```

### Step 2: Request a New Session Challenge (HTTP 402)
```bash
curl -i -X POST http://127.0.0.1:8080/v1/session/new \
  -H "Content-Type: application/json" \
  -d '{"budget_sats": 100}'
```

**Expected Response**:
```http
HTTP/1.1 402 Payment Required
content-type: application/json
www-authenticate: L402 token="MDAxM2xvY2F0...", invoice="lnbcrt100u1pn25870..."

{"error":"Payment required"}
```

### Step 3: Pay the BOLT11 Invoice
Pay the invoice using your Lightning wallet.
- **Polar LND Regtest Example**:
  ```bash
  # Pay invoice from payer node 'alice' (in our Polar environment: polar-n3-alice)
  docker exec -it polar-n3-alice lncli --network=regtest payinvoice --force lnbcrt100u1pn25870...
  ```
  The wallet outputs the payment hash and `payment_preimage`:
  ```json
  {
    "payment_preimage": "e4b2d1844ad0682fa2c0...32byteshex...",
    "payment_hash": "..."
  }
  ```
- **Note on Integration State**:
  - The L402 challenge parsing, header formatting, and state machine behaviors in these examples have been verified by unit and regression test suites.
  - The complete end-to-end payment settlement in this guide demonstrates the standard external developer integration flow with a live Lightning payer node (`polar-n3-alice`) communicating with the SatsGrid node.

### Step 4: Execute Chat Completion (Non-Streaming)
```bash
MACAROON="MDAxM2xvY2F0..."
PREIMAGE="e4b2d1844ad0682fa2c0..."

curl -i -X POST http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: L402 ${MACAROON}:${PREIMAGE}" \
  -d '{
    "model": "llama3.2",
    "messages": [
      {"role": "system", "content": "You are a helpful assistant."},
      {"role": "user", "content": "Explain Lightning Network in one sentence."}
    ],
    "stream": false
  }'
```

**Response Headers**:
```http
HTTP/1.1 200 OK
content-type: application/json
x-satsgrid-charged-sats: 10
x-satsgrid-remaining-budget-sats: 90
x-satsgrid-receipt-id: 3b2f5197-3c31-40d1-8b9f-599a3b8c03bc
```

---

## 7. Python Integration Example

Below is a Python script demonstrating how an application or autonomous agent handles L402 challenges and streams AI inference.

> **Testing Status**:
> The L402 challenge parsing logic, auth header composition, and error models in this example are verified by unit tests (`tests/l402.test.ts` and `tests/client_integration_tests.rs`). The end-to-end payment step (`pay_lightning_invoice`) represents an external developer wallet integration where the caller's Lightning client or node supplies the preimage upon invoice settlement.

```python
"""
SatsGrid Python SDK Integration Example.
Requirements: pip install httpx
"""

import re
import json
import httpx

BASE_URL = "http://127.0.0.1:8080"
MODEL = "llama3.2"

def parse_l402_challenge(authenticate_header: str) -> dict:
    """Extracts token (macaroon) and invoice from WWW-Authenticate header."""
    token_match = re.search(r'token="([^"]+)"', authenticate_header)
    invoice_match = re.search(r'invoice="([^"]+)"', authenticate_header)
    
    if not token_match or not invoice_match:
        raise ValueError(f"Malformed L402 challenge header: {authenticate_header}")
        
    return {
        "macaroon": token_match.group(1),
        "invoice": invoice_match.group(1)
    }

def pay_lightning_invoice(bolt11: str) -> str:
    """
    Simulate or call your wallet backend (Polar, LND, NWC, Core Lightning).
    Must return the 64-character hex payment preimage.
    """
    # In production, call your wallet or NWC client (e.g. nwc.pay_invoice(bolt11))
    print(f"\n[Lightning Wallet] Pay this BOLT11 invoice:\n{bolt11}\n")
    preimage = input("Enter 64-character hex payment preimage: ").strip()
    return preimage

def main():
    client = httpx.Client(base_url=BASE_URL, timeout=30.0)

    # 1. Initialize session budget (100 sats)
    print("1. Requesting new session budget (100 sats)...")
    res = client.post("/v1/session/new", json={"budget_sats": 100})
    
    if res.status_code != 402:
        raise RuntimeError(f"Expected HTTP 402, got {res.status_code}: {res.text}")
        
    challenge = parse_l402_challenge(res.headers["WWW-Authenticate"])
    macaroon = challenge["macaroon"]
    invoice = challenge["invoice"]
    
    print(f"✓ Challenge received.")
    print(f"  Macaroon: {macaroon[:20]}...")
    print(f"  Invoice:  {invoice[:25]}...")

    # 2. Settle the invoice
    preimage = pay_lightning_invoice(invoice)
    auth_header = f"L402 {macaroon}:{preimage}"

    # 3. Stream chat completion
    print("\n2. Dispatching streaming inference request...")
    payload = {
        "model": MODEL,
        "messages": [
            {"role": "user", "content": "Write a 3-line haiku about Bitcoin speed."}
        ],
        "stream": True
    }

    with client.stream(
        "POST",
        "/v1/chat/completions",
        headers={"Authorization": auth_header},
        json=payload
    ) as response:
        if response.status_code != 200:
            print(f"Inference failed (HTTP {response.status_code}): {response.read().decode('utf-8')}")
            return

        print("✓ Response stream opened:")
        charged = response.headers.get("x-satsgrid-charged-sats")
        remaining = response.headers.get("x-satsgrid-remaining-budget-sats")
        receipt_id = response.headers.get("x-satsgrid-receipt-id")

        for line in response.iter_lines():
            line = line.strip()
            if not line or line.startswith(":"):
                continue
            if line == "data: [DONE]":
                break
            if line.startswith("data: "):
                chunk_json = json.loads(line[6:])
                content = chunk_json["choices"][0]["delta"].get("content", "")
                print(content, end="", flush=True)

        print("\n\n--- Session Accounting ---")
        print(f"Charged:   {charged} sats")
        print(f"Remaining: {remaining} sats")
        if receipt_id:
            print(f"Receipt ID: {receipt_id} (Verify at /v1/receipts/{receipt_id})")

if __name__ == "__main__":
    main()
```

---

## 8. Node.js / TypeScript Integration Example

A modern TypeScript integration using native `fetch`:

> **Testing Status**:
> The regex-based header parser, session budget flow, and header structures match the verified frontend client implementation (`gui/client/src/lib/api/client.ts` and `gui/client/tests/l402.test.ts`). Prompting for the payment preimage reflects the external wallet payment step in developer workflows.

```typescript
import { createInterface } from "readline/promises";
import { stdin as input, stdout as output } from "process";

const BASE_URL = "http://127.0.0.1:8080";
const MODEL = "llama3.2";

interface L402Challenge {
  macaroon: string;
  invoice: string;
}

function parseL402Header(header: string): L402Challenge {
  const tokenMatch = header.match(/token="([^"]+)"/);
  const invoiceMatch = header.match(/invoice="([^"]+)"/);

  if (!tokenMatch || !invoiceMatch) {
    throw new Error(`Invalid L402 challenge header: ${header}`);
  }

  return {
    macaroon: tokenMatch[1],
    invoice: invoiceMatch[1],
  };
}

async function runInference() {
  // 1. Request new session challenge
  const challengeRes = await fetch(`${BASE_URL}/v1/session/new`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ budget_sats: 100 }),
  });

  if (challengeRes.status !== 402) {
    throw new Error(`Expected HTTP 402, received ${challengeRes.status}`);
  }

  const authHeader = challengeRes.headers.get("www-authenticate");
  if (!authHeader) throw new Error("Missing WWW-Authenticate header");
  const { macaroon, invoice } = parseL402Header(authHeader);

  console.log(`[Invoice] Pay via Lightning:\n${invoice}\n`);

  // Prompt developer for payment preimage
  const rl = createInterface({ input, output });
  const preimage = (await rl.question("Enter 64-char hex preimage: ")).trim();
  rl.close();

  // 2. Execute chat completion with L402 credentials
  const response = await fetch(`${BASE_URL}/v1/chat/completions`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `L402 ${macaroon}:${preimage}`,
    },
    body: JSON.stringify({
      model: MODEL,
      messages: [{ role: "user", content: "What is L402 in 20 words?" }],
      stream: false,
    }),
  });

  if (!response.ok) {
    const err = await response.text();
    throw new Error(`Inference rejected (${response.status}): ${err}`);
  }

  const data = await response.json();
  console.log("\n[Assistant Response]:", data.choices[0].message.content);
  console.log("Charged (sats):", response.headers.get("x-satsgrid-charged-sats"));
  console.log("Remaining Budget:", response.headers.get("x-satsgrid-remaining-budget-sats"));
}

runInference().catch(console.error);
```

---

## 9. Response Headers & Cryptographic Receipts

On successful completion of any request, the SatsGrid node emits auditable metadata headers:

| Header | Description | Example |
| :--- | :--- | :--- |
| `X-SatsGrid-Charged-Sats` | Exact satoshis charged for this inference request | `10` |
| `X-SatsGrid-Remaining-Budget-Sats` | Remaining available balance in the session | `90` |
| `x-satsgrid-receipt-id` | Unique UUID of the signed cryptographic receipt | `3b2f5197-3c31-40d1...` |

### Independent Receipt Verification
Anyone can verify a signed inference receipt without credentials:
```bash
curl -s http://127.0.0.1:8080/v1/receipts/<receipt_id> | jq .
```
Receipts are signed by the node's Ed25519 key over canonical RFC 8785 JSON bytes, certifying model execution, prompt/completion tokens, and payment settlement.

---

## 10. Error Handling & Edge Cases

All non-200 responses conform to OpenAI standard error JSON envelopes:
```json
{
  "error": {
    "message": "Missing required field 'model'",
    "type": "invalid_request_error",
    "param": "model",
    "code": "missing_required_field"
  }
}
```

| HTTP Status | Error Type / Message | Cause & Remediation |
| :--- | :--- | :--- |
| `400 Bad Request` | `invalid_request_error` | Missing required fields, invalid model identifier, or malformed JSON payload. Check request format. |
| `401 Unauthorized` | `invalid_request_error` | Preimage hash mismatch or payment replay attempt. Ensure you are using the preimage corresponding to the challenge invoice. |
| `402 Payment Required` | `Session budget exhausted` | The session balance has dropped below the request cost. Start a new session with `/v1/session/new`. |
| `403 Forbidden` | `Missing or invalid capability` | The macaroon does not contain `capability = inference` or is restricted to a different model. |
| `404 Not Found` | `Model not found` | The requested model is not enabled or registered on this node. Query `/v1/models`. |
| `429 Too Many Requests` | `rate_limit_error` | Per-session or global concurrency limits reached. Retry with exponential backoff. |

---

## 11. Security Considerations

1. **Protect Preimages and Macaroons in Storage**:
   - The combined `macaroon:preimage` credential acts as an active bearer token granting authorization to spend the session's budget.
   - Do not commit preimages or session credentials to public git repositories.
2. **Never Expose Admin Credentials**:
   - Admin tokens (used for `/v1/node/config` and `/v1/node/stats`) must only exist in secure backend environments.
   - Inference endpoints (`/v1/chat/completions`) **never** require or accept admin tokens.
3. **Frontend Implementation Rule**:
   - Frontend web applications should either hold credentials in memory/session storage or leverage WebLN (`window.webln`) to settle invoices directly in the user's browser extension.

---

## 12. Polar Regtest vs. Mainnet Environments

| Feature | Polar Regtest | Mainnet |
| :--- | :--- | :--- |
| **Invoice Prefix** | `lnbcrt...` | `lnbc...` |
| **Node Base URL** | `http://127.0.0.1:8080` | `https://your-node-domain.org` |
| **Payment Settlement** | Instant via Polar UI or `lncli` | Settled through public Lightning gossip & channels |
| **Preimage Source** | Polar terminal output or LND REST API | Wallet payment receipt or NWC response |
| **Mock Backend Support**| Supported via `/internal/mock/pay` | Disabled for real financial security |
