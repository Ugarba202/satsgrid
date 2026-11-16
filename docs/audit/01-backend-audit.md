# SATSGRID PHASE 0 AUDIT: 01 - BACKEND AUDIT

**Project:** SatsGrid  
**Audit Phase:** Phase 0 Inherited Codebase Audit  
**Classification:** SOURCE VERIFIED & TEST VERIFIED  
**Date:** 2026-11-05  

---

## 1. Executive Summary of Backend Subsystems

The SatsGrid backend is an asynchronous Rust application built on Tokio and Axum. It acts as an L402 payment gate, reverse proxy, and node daemon.

```
                           +---------------------------+
                           |  CLI Dispatcher (clap)    |
                           +---------------------------+
                                         |
                       +-----------------+-----------------+
                       |                                   |
         +---------------------------+       +---------------------------+
         |     SatsGridServer        |       |       SatsGridClient      |
         +---------------------------+       +---------------------------+
           /           |           \                       |
          v            v            v                      v
     +---------+ +-----------+ +----------+       +-------------------+
     | Axum    | | L402 Gate | | Nostr    |       | Client Budget     |
     | Routes  | | & Macaroon| | Discovery|       | Tracker & PayHandler
     +---------+ +-----------+ +----------+       +-------------------+
          |            |
          v            v
     +---------+ +-----------+
     | Upstream| | Lightning |
     | Proxy   | | Drivers   |
     +---------+ +-----------+
```

---

## 2. Detailed Subsystem Audit

### 2.1 CLI and Command Parsing
- **Responsibilities:** Parses command line arguments, handles `node` management, `call` execution, and `discover` queries.
- **Source Files:** `src/cli/{mod.rs, node.rs, call.rs, discover.rs, process.rs}`.
- **Runtime Behavior:** Uses `clap 4.5` derive macros. Subcommands: `satsgrid node start|stop|status|announce`, `satsgrid call`, `satsgrid discover`.
- **Dependencies:** `tokio`, `clap`, `reqwest`, `nostr-sdk`.
- **Test Coverage:** `tests/cli_tests.rs` (2 tests: `--help` and `node status`).
- **Defects / Limitations:** PID management uses plain PID file in `.satsgrid/node.pid`. If process is SIGKILLed, stale PID is detected and cleaned on next start.
- **Verification Status:** **TEST VERIFIED**.

### 2.2 Node Lifecycle, Server, and Routes
- **Responsibilities:** Initializes configuration, manages root macaroon keys, admin tokens, constructs Axum router, and listens on configured TCP port with graceful shutdown.
- **Source Files:** `src/node/server.rs`, `src/node/api/{mod.rs, routes.rs, handlers.rs}`.
- **Runtime Behavior:** Binds `tokio::net::TcpListener`, hooks `SIGINT` (Ctrl+C) and `SIGTERM`.
  - Routes:
    - `GET /health` -> `health_check`
    - `GET /v1/models` -> `models`
    - `POST /v1/session/new` -> `new_session`
    - `POST /v1/chat/completions` -> `chat_completions`
    - `GET /v1/node/stats` -> `node_stats` (Admin required)
    - `GET /v1/node/config` & `POST /v1/node/config` -> `node_config` / `update_node_config` (Admin required)
    - `POST /internal/mock/pay` -> `mock_pay` (Mock backend only)
- **Dependencies:** `axum 0.8`, `tower-http 0.6`.
- **Test Coverage:** `tests/api_server_tests.rs` (10 tests).
- **Verification Status:** **TEST VERIFIED**.

### 2.3 L402 Challenge, Macaroons & Verification
- **Responsibilities:** Issues HTTP 402 challenges (`WWW-Authenticate: L402 token="...", invoice="..."`), mints macaroons using HMAC-SHA256, verifies signatures, caveat predicates, and preimage hashes.
- **Source Files:** `src/node/gate/{macaroon.rs, challenge.rs, verify.rs}`.
- **Runtime Behavior:**
  - Macaroon identifier = `payment_hash.0`.
  - Signature computed by chaining HMAC-SHA256 starting from `macaroon_key` (persisted in `.satsgrid/.satsgrid_macaroon_key`).
  - Verifier checks:
    1. `sha256(preimage) == identifier` (Hex lowercase match).
    2. Signature integrity over identifier and all caveats.
    3. Expiration caveat (`time < <unix_ts>`).
    4. Model restriction caveat (`model = <model_name>`).
    5. Capability caveat (`capability = "inference"`).
    6. Lightning backend `is_invoice_settled(identifier)`.
- **Test Coverage:** 14 unit tests in `gate/macaroon.rs`, 8 unit tests in `gate/challenge.rs`, 8 unit tests in `gate/verify.rs`, `tests/gate_integration_tests.rs`.
- **Verification Status:** **TEST VERIFIED**.

### 2.4 Pricing and Token Estimation
- **Responsibilities:** Computes Satoshi cost for requests based on base fees and token rates.
- **Source Files:** `src/node/pricing.rs`.
- **Runtime Behavior:**
  - Base fee: `config.pricing.default_price_sats` (default 10 sats).
  - Prompt tokens estimated via character count heuristic: `text.trim().len().div_ceil(4)`.
  - Completion tokens estimated strictly from `max_tokens` payload attribute. If omitted, completion cost = 0.
  - Formula: $\text{Base} + (\text{prompt\_tokens} \times \text{prompt\_rate}) + (\text{completion\_tokens} \times \text{completion\_rate})$.
- **Test Coverage:** 5 unit tests in `src/node/pricing.rs`.
- **Verification Status:** **TEST VERIFIED**.

### 2.5 Session Budget Management
- **Responsibilities:** Tracks caller balances for multi-turn chat sessions without requiring per-request Lightning invoices.
- **Source Files:** `src/node/gate/budget.rs`.
- **Runtime Behavior:** Lock-free Compare-And-Swap (`AtomicU64::compare_exchange_weak`) in a retry loop. Debits `cost` from `remaining_sats`.
- **Test Coverage:** 6 unit tests in `gate/budget.rs`, `tests/client_budget_tests.rs` (3 tests).
- **Critical Limitation:** Sessions live exclusively in RAM (`HashMap` in `SessionBudgetManager`). Daemon restart destroys all active session balances.
- **Verification Status:** **TEST VERIFIED**.

### 2.6 OpenAI Reverse Proxy & Streaming
- **Responsibilities:** Forwards chat completion payloads to the upstream model engine and returns responses (JSON or SSE stream).
- **Source Files:** `src/node/proxy/openai.rs`.
- **Runtime Behavior:** Strips incoming `Authorization` header to prevent leaking node credentials. Forwards headers to `{upstream_url}/v1/chat/completions`.
  - When `stream: true`, pipes upstream `bytes_stream()` into Axum `text/event-stream`.
- **Test Coverage:** `tests/proxy_openai_tests.rs` (8 tests).
- **Verification Status:** **TEST VERIFIED**.

### 2.7 Lightning Drivers (Mock, LND, NWC)
- **Responsibilities:** Provides pluggable payment backend abstraction via `LightningBackend` trait.
- **Source Files:** `src/node/lightning/{backend.rs, invoice.rs, mock.rs, lnd.rs, nwc.rs}`.
- **Runtime Behavior:**
  - `MockLightningBackend`: In-memory thread-safe hashmap storing preimages and settled flags.
  - `LndBackend`: REST HTTPS driver interacting with LND's `/v1/invoices`, `/v1/invoice/{hash}`, and `/v1/channels/transactions`.
  - `NwcLightningBackend`: NIP-47 Nostr Wallet Connect client issuing `make_invoice`, `lookup_invoice`, and `pay_invoice`.
- **Test Coverage:** `tests/mock_lightning_tests.rs`, `tests/lnd_backend_tests.rs` (7 tests), `tests/nwc_backend_tests.rs` (6 tests).
- **Verification Status:** **TEST VERIFIED**.

### 2.8 Nostr Discovery (NIP-89)
- **Responsibilities:** Broadcasts node presence, models, and pricing to Nostr relays; allows clients to query for active nodes.
- **Source Files:** `src/discovery/{announcement.rs, client.rs}`.
- **Runtime Behavior:** Builds and signs Nostr Kind 31990 events with identifier `"satsgrid-node"`, hashtag `"satsgrid"`, endpoint, pricing, and `"m"` model tags.
- **Test Coverage:** 3 unit tests in `announcement.rs`, 1 in `client.rs`, `tests/discovery_tests.rs` (7 tests).
- **Verification Status:** **TEST VERIFIED**.

### 2.9 Client SDK (`SatsGridClient`)
- **Responsibilities:** Caller SDK for programmatic inference consumption with automatic 402 challenge resolution.
- **Source Files:** `src/client/{lib.rs, budget.rs, error.rs, openai.rs, pay.rs}`.
- **Runtime Behavior:** Provides `SatsGridClientBuilder`. Handles session creation (`/v1/session/new`), invoice payment via injected `LightningPaymentProvider`, and caching of the L402 authorization header.
- **Test Coverage:** `tests/client_builder_tests.rs` (3 tests), `tests/client_pay_tests.rs` (3 tests), `tests/client_integration_tests.rs` (5 tests).
- **Verification Status:** **TEST VERIFIED**.

---

## 3. Major Backend Defect & Gap Analysis

| Subsystem | Observed Flaw | Severity | Code Location |
| :--- | :--- | :--- | :--- |
| **Authentication** | **Pay-Per-Request Replay:** The node does not track spent non-session invoices. A paid single-use token can be reused for 1 hour. | **CRITICAL** | `src/node/api/handlers.rs:198-216` |
| **Settlement** | **Payment Before Inference:** Lightning payment or session debit settles *before* contacting the upstream engine. If inference fails, sats are lost. | **CRITICAL** | `src/node/api/handlers.rs:207-238` |
| **Session Budgets** | **Volatile RAM State:** Session budgets are wiped on restart. No database backing. | **HIGH** | `src/node/gate/budget.rs:59-69` |
| **Pricing** | **Inaccurate Token Accounting:** Token usage is estimated via heuristics; actual upstream provider usage metadata is ignored. | **MEDIUM** | `src/node/pricing.rs:48` |
| **Network Limits** | **Unbounded JSON Payloads:** Axum has no explicit `DefaultBodyLimit` on chat completions, risking memory exhaustion. | **MEDIUM** | `src/node/api/handlers.rs:104` |

