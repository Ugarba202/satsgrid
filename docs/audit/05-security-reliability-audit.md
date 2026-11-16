# Phase 0 Audit: Security, Threat Modeling & Reliability Audit

**Status:** SOURCE VERIFIED  
**Auditor Focus:** Cryptography, L402 Implementation, State Management, Upstream Execution  

---

## 1. Threat Modeling Overview

SatsGrid operates as an unauthenticated, internet-facing proxy accepting micropayments over Bitcoin Lightning to gate access to local LLM computing resources. This architecture faces distinct economic, cryptographic, and operational attack vectors.

```
                    ┌───────────────────────────┐
                    │       External Caller     │
                    └─────────────┬─────────────┘
                                  │ (HTTP / L402)
                                  ▼
                    ┌───────────────────────────┐
                    │  SatsGrid L402 Gate       │  <--- [VULN 1: Token Replay]
                    │  (src/l402/verifier.rs)   │  <--- [VULN 3: Unbounded Payloads]
                    └─────────────┬─────────────┘
                                  │ (Pre-debited)
                                  ▼
                    ┌───────────────────────────┐
                    │  Session / Payment Engine │  <--- [VULN 2: Volatile RAM State]
                    │  (src/node/session.rs)    │  <--- [VULN 4: Settlement Before Exec]
                    └─────────────┬─────────────┘
                                  │ (HTTP Request)
                                  ▼
                    ┌───────────────────────────┐
                    │ Upstream Inference Engine │
                    │ (Ollama / vLLM / GPU)     │
                    └───────────────────────────┘
```

---

## 2. In-Depth Vulnerability Analysis

### 2.1 CRITICAL: Pay-Per-Request Token Replay
- **Location:** `src/l402/verifier.rs` (`verify_l402_credentials`), `src/proxy/handler.rs`.
- **Finding:** In pay-per-request mode, a caller requests inference, receives an invoice for e.g. 10 sats, settles the invoice, and presents the macaroon + preimage.
- **Verification Logic:**
  1. The verifier checks HMAC-SHA256 signature validity.
  2. The verifier checks caveat `time < expiry` (defaults to 3,600 seconds / 1 hour).
  3. The verifier queries the Lightning backend: `backend.is_invoice_settled(hash)`.
  4. The Lightning backend returns `Ok(true)` because the invoice was indeed paid.
- **Defect:** **There is no nullifier database or spent-token registry.**
- **Impact:** An attacker who pays 10 sats once can replay the exact same `Authorization: L402 <macaroon>:<preimage>` header thousands of times across the entire 1-hour window to extract unlimited compute for free.
- **Severity:** `CRITICAL` (Direct economic loss for node operators).

### 2.2 CRITICAL: Payment Settlement Prior to Upstream Execution
- **Location:** `src/proxy/handler.rs` (`handle_chat_completions`).
- **Finding:** When an inference request arrives:
  1. The L402 gate validates the macaroon and preimage.
  2. For sessions, the budget is debited in `SessionManager` *before* the upstream HTTP call is made.
  3. The proxy dispatches the prompt to `http://127.0.0.1:11434/v1/chat/completions`.
- **Defect:** If the upstream engine crashes, encounters an Out-Of-Memory (OOM) error, or terminates the connection during prompt processing, the satoshis have already been settled and deducted.
- **Impact:** The caller loses funds without receiving the model output, and the node provides no refund or credit mechanism.
- **Severity:** `CRITICAL` (Direct financial penalty for honest callers).

### 2.3 HIGH: Ephemeral In-Memory State & Node Crash Vulnerability
- **Location:** `src/node/session.rs` (`SessionManager`).
- **Finding:** Active sessions, pre-funded balances, and spend metrics are stored in a `tokio::sync::RwLock<HashMap<String, Session>>`.
- **Defect:** There is zero persistence (no SQLite, RocksDB, or disk write-ahead log).
- **Impact:** If the node operator's machine restarts, power-cycles, or updates the binary:
  - All funded session budgets are instantly erased.
  - Callers holding valid macaroons are permanently locked out and cannot recover unspent satoshis.
  - Node operator metrics (total sats earned, total requests) reset to 0.
- **Severity:** `HIGH` (Reliability and financial loss across restarts).

### 2.4 MEDIUM: Unbounded Request Body Parsing & DoS
- **Location:** `src/proxy/handler.rs` (`handle_chat_completions(Json(req))`).
- **Finding:** The endpoint parses incoming JSON payloads directly with Axum's default `Json` extractor without wrapping the body stream in `RequestBodyLimitLayer`.
- **Impact:** An unauthenticated or single-request attacker can send a multi-gigabyte JSON payload in the request body, consuming node RAM and exhausting memory before the L402 gate or model logic is reached.
- **Severity:** `MEDIUM` (Denial of Service).

### 2.5 MEDIUM: Unauthenticated Relay Discovery Trust Model
- **Location:** `src/nostr/discovery.rs`.
- **Finding:** Any Nostr keypair can publish Kind 31990 announcements tagging `satsgrid` with arbitrary `node_url` endpoints, fake model names, and arbitrary prices.
- **Impact:** A malicious actor can flood Nostr relays with bogus nodes, directing client traffic to malicious honeypots or unavailable endpoints. Callers currently do not verify operator reputation, stake, or proof-of-work.
- **Severity:** `MEDIUM` (Discovery poisoning).

---

## 3. Cryptographic Implementation Review

| Primitive | Implementation | Status | Notes |
|---|---|---|---|
| **Macaroon Construction** | `hmac::Hmac<sha2::Sha256>` | SECURE | Chained HMAC-SHA256 correctly binds caveats to root key. |
| **Preimage Verification** | `sha2::Sha256::digest(preimage)` | SECURE | Verifies `SHA256(preimage) == payment_hash`. Constant-time comparison is recommended. |
| **Caveat Parsing** | First-party strings (`k = v`) | FUNCTIONAL | Simple key-value caveats. Lack third-party caveats (not needed for Phase 0). |
| **Root Key Lifecycle** | Node config / RAM generated | CAUTION | If root key is generated ephemerally, macaroons invalidate upon restart. |
