# Phase 0 Audit: Technical Baseline & Audit Reconciliation Report

**Auditor:** Senior Rust Software Architect & Security Auditor  
**Document:** `docs/audit/08-audit-reconciliation.md`  
**Date:** 2026-11-05  
**Target Repository:** `/home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid`  
**Mode:** READ-ONLY AUDIT RECONCILIATION — ZERO CODE MODIFICATIONS  

---

## 1. Executive Summary

This report establishes the verified single source of truth for the SatsGrid codebase by reconciling conflicting statements found across earlier Phase 0 audit documents (`docs/audit/00` through `07`). Every claim in this document is directly grounded in source code inspection, exact command outputs, or verified execution logs.

### Key Baseline Reconciliations:
1. **Web Framework Version:** Axum is **`0.8`** (specifically `0.8.1` in `Cargo.lock`), **not `0.7`**.
2. **Lightning LND Driver:** LND backend communicates strictly via **REST HTTP API** using `reqwest` and `Grpc-Metadata-macaroon` headers, **not gRPC**.
3. **Internal Module Paths:** The L402 gate logic resides in **`src/node/gate/`**, proxy logic in **`src/node/proxy/`**, and Lightning drivers in **`src/node/lightning/`**. Reports citing `src/l402/`, `src/proxy/`, or `src/lightning/` referenced nonexistent top-level paths.
4. **Rust Test Suite:** Exactly **17 test targets/binaries** execute; **15 test suites** contain active tests; exactly **102 tests run and pass (0 failed, 0 ignored)**.
5. **Frontend Lint vs. Build:** `npm run build` succeeds completely (exit code `0`, static routes generated). `npm run lint` executes ESLint 9 against Next.js 16 core web vitals and strict TypeScript-eslint rules, reporting **1,570 errors and 33,386 warnings** and exiting with code **`1`**.
6. **Docker Compose Upstream Routing:** In `docker/docker-compose.yml`, the node mounts `../config/node.example.toml:ro`, which sets `upstream.url = "http://127.0.0.1:11434"`. Inside container isolation, `127.0.0.1` targets the node container itself rather than the `satsgrid-ollama` container (`http://ollama:11434`), preventing upstream communication in containerized setups unless overridden.
7. **Production Critical Vulnerabilities:** CONFIRMED that **pay-per-request tokens can be replayed infinitely for 1 hour** (no nullifier registry exists), **payment settlement occurs prior to model execution** (no refunds if Ollama fails), and **all session budgets exist in volatile RAM** (erased on restart).

---

## 2. Verified Repository Structure & Module Map

### 2.1 Actual Source Layout (`src/`)
```
src/
├── cli/                        # CLI subcommands: call, discover, node, process
├── client/                     # Rust client SDK: builder, budget tracker, openai, pay
├── common/                     # Core domain primitives: error, types (Satoshis, Preimage, etc.)
├── config/                     # TOML schema and loader: NodeConfig, ClientConfig
├── discovery/                  # Nostr NIP-89 discovery client & announcement
├── lib.rs                      # Library root exporting client, config, common
├── main.rs                     # CLI binary entrypoint (satsgrid)
└── node/                       # Node daemon subsystems
    ├── api/                    # Axum routes & handlers: /health, /v1/chat/completions, /v1/models, /v1/node/*
    ├── config.rs               # Node-specific runtime config
    ├── gate/                   # L402 payment gate, HMAC-SHA256 macaroons, verifier, session budgets
    ├── lightning/              # Backend abstraction, Mock, LND (REST), NWC (NIP-47)
    ├── pricing.rs              # Dynamic satoshi pricing & prompt token estimation
    ├── proxy/                  # OpenAI & Ollama HTTP proxy and SSE delta streamer
    └── server.rs               # Node lifecycle, graceful shutdown, key/token generation
```

### 2.2 Reconciled Module Path Table

| Logical Capability | Nonexistent Path (Earlier Reports) | Actual Verified Source Path |
|---|---|---|
| L402 Gate & Verifier | `src/l402/verifier.rs`, `src/l402/macaroon.rs` | `src/node/gate/verify.rs`, `src/node/gate/macaroon.rs` |
| Session Budgeting | `src/node/session.rs` | `src/node/gate/budget.rs` |
| Proxy & Streamer | `src/proxy/handler.rs` | `src/node/proxy/openai.rs`, `src/node/api/handlers.rs` |
| Lightning Drivers | `src/lightning/lnd.rs`, `src/lightning/mock.rs` | `src/node/lightning/lnd.rs`, `src/node/lightning/mock.rs` |
| Nostr Discovery | `src/nostr/discovery.rs` | `src/discovery/client.rs`, `src/discovery/announcement.rs` |

---

## 3. Dependency & Framework Baseline

Verified from `Cargo.toml` and `Cargo.lock`:

| Dependency | Declared in `Cargo.toml` | Resolved in `Cargo.lock` | Audit Reconciliation Notes |
|---|---|---|---|
| **Rust Edition** | `2021` | `2021` | No minimum supported Rust version (`rust-version`) declared in `[package]`. Tested on `rustc 1.85.0`. |
| **Axum** | `0.8` (features: `macros`) | `0.8.1` | **CONFIRMED 0.8**. Earlier claims of Axum 0.7 were inaccurate. |
| **Tokio** | `1.43` (features: `full`) | `1.43.0` | Async multi-threaded runtime. |
| **Tower HTTP** | `0.6` (features: `cors`, `trace`) | `0.6.2` | Middleware for tracing and CORS. |
| **Reqwest** | `0.12` (rustls-tls, json, stream) | `0.12.12` | Used for upstream inference proxy and LND REST calls. |
| **Nostr SDK** | `0.45.4` | `0.45.4` | Nostr relay communication. |
| **Nostr** | `0.45.5` (features: `nip47`) | `0.45.5` | NIP-47 protocol structures. |
| **NWC** | `0.45.1` | `0.45.1` | Nostr Wallet Connect client. |
| **Database** | *None* | *None* | **CONFIRMED:** No SQL, SQLite, or key-value database dependency exists. |

---

## 4. Test Suite Execution & Verification

### 4.1 Exact Execution Details
- **Command:** `cargo test --workspace`
- **Exit Code:** `0` (Success)
- **Compile Warnings:** None.
- **Total Test Binaries/Targets Executed:** 17
- **Suites Containing Active Tests:** 15
- **Tests Passed:** **102**
- **Tests Failed:** **0**
- **Tests Ignored:** **0**
- **Filtered Out:** **0**

### 4.2 Exact Suite Breakdown

| # | Test Target / Binary | Passed | Failed | Ignored | Subsystems Tested |
|---|---|:---:|:---:|:---:|---|
| 1 | `unittests src/lib.rs` | 40 | 0 | 0 | Macaroon HMAC, caveats, challenge parsing, L402 verifier, pricing estimation, budget manager, NWC URI parsing, Nostr tags. |
| 2 | `unittests src/main.rs` | 0 | 0 | 0 | Binary harness. |
| 3 | `tests/api_server_tests.rs` | 10 | 0 | 0 | `/health`, `/v1/models`, `/v1/chat/completions`, admin security on `/v1/node/*`, session budget debits. |
| 4 | `tests/cli_tests.rs` | 2 | 0 | 0 | CLI `--help` and `node status`. |
| 5 | `tests/client_budget_tests.rs` | 3 | 0 | 0 | Client-side budget tracking and CAS concurrency. |
| 6 | `tests/client_builder_tests.rs` | 3 | 0 | 0 | Builder validation, required fields, fallback defaults. |
| 7 | `tests/client_integration_tests.rs` | 5 | 0 | 0 | End-to-end client connection, SSE stream handling, session expiration. |
| 8 | `tests/client_pay_tests.rs` | 3 | 0 | 0 | L402 challenge parsing and payment dispatching. |
| 9 | `tests/config_tests.rs` | 5 | 0 | 0 | TOML config parsing, defaults, environment variable overrides. |
| 10 | `tests/discovery_tests.rs` | 7 | 0 | 0 | Nostr announcement serialization, tag parsing, filter construction. |
| 11 | `tests/gate_integration_tests.rs` | 1 | 0 | 0 | L402 handshake round-trip and budget lifecycle. |
| 12 | `tests/lnd_backend_tests.rs` | 7 | 0 | 0 | LND REST invoice creation, payment status checking, network configuration. |
| 13 | `tests/mock_lightning_tests.rs` | 1 | 0 | 0 | Mock backend invoice creation and payment settlement. |
| 14 | `tests/nwc_backend_tests.rs` | 6 | 0 | 0 | NWC URI validation, config deserialization, trait implementation. |
| 15 | `tests/privacy_audit_tests.rs` | 1 | 0 | 0 | Verification that prompt contents are omitted from tracing spans. |
| 16 | `tests/proxy_openai_tests.rs` | 8 | 0 | 0 | Forwarding to mock upstream, header filtering, streaming SSE delta, timeout handling. |
| 17 | `Doc-tests satsgrid` | 0 | 0 | 0 | Library documentation tests. |
| **—** | **TOTAL** | **102** | **0** | **0** | **All 102 tests pass cleanly.** |

---

## 5. Frontend Build and Lint Reconciliation

### 5.1 Build Verification
- **Command:** `cd gui/client && npm run build`
- **Exit Code:** `0` (Success)
- **Tooling:** Next.js 16.3.6 (Turbopack), React 19.2.8, TypeScript 5.9.3.
- **Output:** Successfully compiled static pages for `/`, `/node`, `/playground`, `/sessions`, and `/_not-found`.

### 5.2 Lint Reconciliation
- **Command:** `cd gui/client && npm run lint` (`eslint`)
- **Exit Code:** **`1` (Failure)**
- **Reconciliation Resolution:**
  - One earlier audit noted exit code 0 because the process completed its scan; however, ESLint exits with code **`1`** because errors were flagged.
  - **Total Problems:** **34,956**
    - **Errors:** **1,570**
    - **Warnings:** **33,386**
  - **Root Cause:** Next.js 16.3.6 ships with ESLint flat config (`eslint.config.mjs`) enabling React 19 `core-web-vitals` rules (e.g. `react-hooks/set-state-in-effect`, forbidding synchronous `setState` calls inside `useEffect`) and strict TypeScript-eslint rules (e.g. `@typescript-eslint/no-explicit-any`, `@typescript-eslint/ban-ts-comment`).
  - **Verdict:** The frontend builds and runs cleanly in production mode, but fails strict ESLint CI validation under Next.js 16 defaults.

---

## 6. Lightning Implementation Findings

Source inspection of `src/node/lightning/`:

### 6.1 LND Backend (`src/node/lightning/lnd.rs`) — CONFIRMED REST
- **Protocol:** Uses **REST HTTP/HTTPS via `reqwest::Client`**, NOT gRPC.
- **Authentication:** Macaroon hex string sent as HTTP header `Grpc-Metadata-macaroon: <hex>`.
- **Endpoints Called:**
  - `POST /v1/invoices` (creates invoice; returns `payment_request` and base64 `r_hash`).
  - `GET /v1/invoice/{payment_hash}` (checks settlement state via `settled` bool or `state == "SETTLED"`).
  - `POST /v1/channels/transactions` (pays outgoing invoice; extracts `payment_preimage`).
- **Hold Invoice Support:** **NOT SUPPORTED.** The LND implementation does not call `/v2/invoices/hodl` or subscribe to invoice state streams.

### 6.2 NWC Backend (`src/node/lightning/nwc.rs`) — CONFIRMED NIP-47
- **Protocol:** Nostr Wallet Connect client via `nwc` crate.
- **Supported Methods:**
  - `make_invoice`: Requests invoice creation with amount in millisatoshis.
  - `lookup_invoice`: Queries invoice settlement status via `settled_at.is_some()`.
  - `pay_invoice`: Dispatches payment request and extracts preimage.
- **Hold Invoices:** **NOT SUPPORTED** by NIP-47 specifications.

### 6.3 Mock Backend (`src/node/lightning/mock.rs`) — CONFIRMED IN-MEMORY
- Generates deterministic simulated invoices (`lnbc{amount}mock{hash}`).
- Settles instantly via internal method or `POST /internal/mock/pay`.

### 6.4 Trait Abstraction (`src/node/lightning/backend.rs`)
```rust
#[async_trait]
pub trait LightningBackend: Send + Sync {
    async fn create_invoice(&self, amount_sats: Satoshis, memo: &str) -> Result<Invoice>;
    async fn is_invoice_settled(&self, payment_hash: &PaymentHash) -> Result<bool>;
    async fn pay_invoice(&self, invoice: &str) -> Result<String>;
}
```
- **Finding:** The trait only supports `create_invoice`, `is_invoice_settled`, and `pay_invoice`. It provides **no methods for hold invoices, escrow locks, cancellation, or settlement releases**.

---

## 7. Security and Reliability Findings

### 7.1 Pay-Per-Request Token Replay — CONFIRMED CRITICAL
- **File:** `src/node/gate/verify.rs` (lines 62–93), `src/node/api/handlers.rs` (lines 176–245).
- **Execution Path:**
  1. Caller submits `Authorization: L402 <macaroon>:<preimage>`.
  2. `L402Verifier::verify_credentials` computes `SHA256(preimage)` and checks macaroon signature.
  3. It verifies caveat `time < expiry` (default +3,600s).
  4. It calls `backend.is_invoice_settled(hash)`.
  5. The backend confirms the invoice was settled.
  6. **There is no nullifier store, spent-hash registry, or uniqueness check.**
- **Verdict:** A single 10-sat payment credential can be replayed an infinite number of times within the 1-hour validity window.

### 7.2 Settlement Before Inference Execution — CONFIRMED CRITICAL
- **File:** `src/node/api/handlers.rs` (lines 170–245).
- **Execution Path:**
  1. The L402 gate validates payment credentials.
  2. For sessions, the budget is debited via `state.budget_manager.debit_session()` *before* invoking Ollama.
  3. `state.stats.total_sats_earned` increments.
  4. The request is forwarded via `state.proxy.forward_chat_completion_with_headers()` or `stream_chat_completion()`.
  5. If the upstream engine crashes, times out, or returns HTTP 500, the caller's satoshis are already gone.
  6. If an SSE stream drops mid-generation, all estimated/charged tokens are forfeited with no rollback.
- **Verdict:** Callers bear 100% of the financial risk of upstream infrastructure failures.

### 7.3 Volatile In-Memory Session State — CONFIRMED HIGH
- **File:** `src/node/gate/budget.rs` (lines 63–115).
- **Execution Path:**
  - Budgets reside in `SessionBudgetManager` backed by `Arc<RwLock<HashMap<String, SessionBudget>>>`.
  - Individual budgets use lock-free `AtomicU64` with CAS (`compare_exchange_weak`), making concurrent debits thread-safe during runtime.
  - **Zero persistence exists.** Any node restart, update, or crash completely destroys all session records, balances, and operator earnings metrics.

### 7.4 Request Body Limits & DoS — CONFIRMED MEDIUM
- **File:** `src/node/api/routes.rs`, `src/node/server.rs`.
- **Finding:** The Axum router attaches `TraceLayer` and `CorsLayer`, but does **not** attach `tower_http::limit::RequestBodyLimitLayer`.
- **Verdict:** An unauthenticated caller can send an arbitrarily large JSON body, forcing the node to buffer multi-gigabyte payloads in memory.

### 7.5 Upstream Token Pricing Accuracy — CONFIRMED MEDIUM
- **File:** `src/node/pricing.rs`, `src/node/api/handlers.rs` (lines 146–160).
- **Finding:**
  - Prompt tokens are estimated heuristically using character count (`len.div_ceil(4)`).
  - Completion tokens are estimated solely from `payload.get("max_tokens")`. If `max_tokens` is omitted by the caller, completion tokens are assumed to be `0`.
  - When the response arrives, the node does not reconcile the heuristic cost against actual `usage.prompt_tokens` or `usage.completion_tokens` returned by Ollama.

---

## 8. Docker Deployment Findings

Source inspection of `docker/Dockerfile` and `docker/docker-compose.yml`:

### 8.1 Docker Compose Configuration
- **File:** `docker/docker-compose.yml`
- **Validation:** `docker compose config` passes with exit code `0`.
- **Services:**
  - `satsgrid-ollama`: Port `11434:11434`, volume `ollama_data:/root/.ollama`.
  - `satsgrid-node`: Port `8080:8080`, depends on `ollama`, volume `../config/node.example.toml:/etc/satsgrid/node.toml:ro`.

### 8.2 The "Localhost" Upstream Network Discrepancy — CONFIRMED
- In `config/node.example.toml`, the default upstream URL is:
  ```toml
  [upstream]
  url = "http://127.0.0.1:11434"
  ```
- Because Docker Compose assigns each container its own network namespace, `127.0.0.1` inside the `satsgrid-node` container refers to the node container itself, not the sibling `satsgrid-ollama` container.
- **Verdict:** Running `docker compose up` without altering `config/node.example.toml` causes all inference proxy requests to fail with `connection refused` on `127.0.0.1:11434`. In Compose, the URL must be `http://ollama:11434` or use host networking.

### 8.3 Security & Privileges
- **Dockerfile:** Multi-stage build (`rust:1.85-slim-bookworm` builder -> `debian:bookworm-slim` runtime).
- **Non-Root Execution:** Provisioned user `satsgrid` (UID 1000) owns `/etc/satsgrid` and `/var/lib/satsgrid`. Runs as `USER satsgrid`.
- **Healthcheck:** Uses `curl -f http://127.0.0.1:8080/health`. Validates HTTP server responsiveness, though it does not verify upstream Ollama connectivity.

---

## 9. Documentation Identity Audit

Search of documentation under `docs/`:

### 9.1 Active Operational Docs Needing Alignment to SatsGrid
The following active operational commands and references still cite legacy identifiers and must be updated in Phase 1:
- `docs/caller-guide.md`: `infernos discover`, `infernos call`, `infernos::client::...` Rust imports, `"service": "infernos"`.
- `docs/node-guide.md`: `./target/release/infernos`, `infernos node start`, `infernos node announce`, `docker exec -it infernos-ollama`.
- `docs/nostr-and-nwc.md`: `INFERNOS_PAYER_NWC_URI`, `INFERNOS_NWC_URI`, `infernos call`, `infernos discover`, `#t="infernos"`, `d="infernos-node"`.
- `docs/gui-architecture-and-audit.md`: `infernos-node` daemon name, `infernos-logo.svg`.

### 9.2 Legal Attribution (Must be Preserved)
- `LICENSE`: `Copyright (c) 2026 Infernos Contributors` (MIT License requirement).
- `Cargo.toml`: `repository = "https://github.com/infernos-ai/infernos"` (upstream git origin).

---

## 10. Audit Contradictions and Resolution Matrix

| Issue | Prior Audit Document Claim | Verified Actual Reality | Final Resolution |
|---|---|---|---|
| **Axum Version** | Claimed `0.7` in report 01 | `Cargo.toml` declares `0.8`, `Cargo.lock` resolves `0.8.1` | **CONFIRMED 0.8.1** |
| **LND Protocol** | Claimed `gRPC` in reports 01 & 06 | `src/node/lightning/lnd.rs` uses `reqwest` REST calls | **CONFIRMED REST API** |
| **Source Directory** | Claimed `src/l402/` and `src/proxy/` in report 05 | Gate is in `src/node/gate/`, proxy is in `src/node/proxy/` | **CONFIRMED `src/node/`** |
| **Rust Test Count** | Varied across early reports (e.g. 70, 85, 102) | `cargo test --workspace` executes 17 targets, 102 active tests | **CONFIRMED EXACTLY 102 PASSED** |
| **Frontend Lint** | Claimed "successful lint" vs "1,570 errors" | Exits with code `1`, 1,570 errors, 33,386 warnings | **CONFIRMED LINT FAILS (EXIT 1)** |
| **Docker Compose** | Assumed functional out of the box | `127.0.0.1:11434` fails in container isolation | **CONFIRMED UPSTREAM CONFIG DRIFT** |
| **Session Atomicity** | Questioned thread safety in early audits | `AtomicU64` with CAS provides atomic debiting in RAM | **CONFIRMED ATOMIC IN-MEMORY** |

---

## 11. Unverified Items & Limitations

1. **Live Bitcoin Lightning Mainnet/Testnet Settlement:**
   - Unit tests verify the Mock backend and mock HTTP responses for LND REST. Actual transaction broadcast and channel state settlement against live Bitcoin nodes were not executed (by strict instruction).
2. **Live Nostr Relay Interaction:**
   - NWC and discovery unit tests verify event serialization, URI parsing, and relay filter generation. Live network WebSocket connections to remote relays (`relay.damus.io`, `nos.lol`) were not tested to preserve offline determinism.

---

## 12. Prioritized Technical Recommendations

1. **Severity 1 (CRITICAL) — Replay Attack Prevention (Nullifier Store):**
   - Add SQLite database with table `spent_tokens(payment_hash PRIMARY KEY, preimage, spent_at)`.
   - In `L402Verifier`, atomically check and record the token before honoring requests.
2. **Severity 1 (CRITICAL) — PayGuard Hold Invoices:**
   - Implement LND Hold Invoices (`/v2/invoices/hodl`) to lock funds in HTLC escrow before calling upstream LLMs, settling only upon successful token generation.
3. **Severity 2 (HIGH) — Persistent Session Budgets:**
   - Migrate `SessionBudgetManager` state to SQLite so session balances survive node restarts.
4. **Severity 2 (HIGH) — Request Body Size Limits:**
   - Add `tower_http::limit::RequestBodyLimitLayer::new(10 * 1024 * 1024)` (10MB) to Axum router.
5. **Severity 3 (MEDIUM) — Docker Compose Upstream Alignment:**
   - Update `docker/docker-compose.yml` to supply environment variable `SATSGRID_UPSTREAM_URL=http://ollama:11434` to the node container.
6. **Severity 3 (MEDIUM) — Documentation Cleanup:**
   - Update the remaining active operational guides in `docs/` from `infernos` to `satsgrid`.
