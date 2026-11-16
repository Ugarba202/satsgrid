# SatsGrid

**AI compute access powered by Bitcoin Lightning.**

SatsGrid is an open, permissionless AI compute platform designed to bridge high-performance open-weight machine learning models with instant, sovereign micro-payments via Bitcoin Lightning and the L402 payment protocol.

Originally built on the technical foundation of the open-source **Infernos** inference engine, SatsGrid evolves this core architecture to address compute accessibility, sovereign payment rails, and permissionless node deployment across emerging markets, with an Africa-first focus.

---

## The Problem SatsGrid Addresses

1. **Global Compute Inequality:** Developers, startups, and researchers in emerging markets frequently encounter severe barriers when trying to access state-of-the-art AI infrastructure—including foreign currency exchange restrictions, credit card processing blocks, and geographically exclusionary cloud providers.
2. **Payment Rail Friction:** Traditional inference APIs enforce recurring fiat subscriptions or require international credit cards, locking out developers who hold sovereign funds in Bitcoin or local digital payment rails.
3. **Compute Monopolies:** Compute providers with spare GPU or NPU capacity lack a frictionless, permissionless mechanism to monetize their hardware globally without complex billing integration or custodial payment intermediaries.

SatsGrid solves this by uniting open-weight inference runtimes with direct, per-request and per-session micro-settlement over the Bitcoin Lightning Network using standard L402 HTTP macaroons.

---

## Product Direction & Architecture

SatsGrid operates as a modular, high-throughput gateway sitting directly between AI clients and local or distributed inference backends:

```
+-------------------------------------------------------------+
|                      SatsGrid Client                        |
|       (CLI / Web UI / Python SDK / Autonomous Agents)        |
+-------------------------------------------------------------+
                              |
                     HTTP / L402 Handshake
                              |
                              v
+-------------------------------------------------------------+
|                    SatsGrid Node Gateway                    |
|  - L402 Macaroon Minting & Verification (HMAC-SHA256)       |
|  - Dynamic Satoshi Pricing & Token Estimation               |
|  - Persistent SQLite Session Budgets & Accounting           |
|  - Single-Use Payment Authorization & Replay Prevention     |
|  - Durable Inference Request Lifecycle State Machine        |
|  - Monitored SSE Stream Delivery with Drop Guards           |
|  - Crash Recovery (Conservative Reconciliation Flagging)    |
|  - Privacy Shield (Zero prompt logging, Header Sanitizing)  |
+-------------------------------------------------------------+
          |                                       |
    Lightning Rails                          Upstream AI
          |                                       |
  +---------------+---------------+        +--------------+
  |               |               |        | OpenAI-API   |
+----+         +-----+         +-----+     | Compatible   |
|Mock|         | LND |         | NWC |     | (Ollama/vLLM)|
+----+         +-----+         +-----+     +--------------+
```

### Core Architecture Capabilities

- **L402 Protocol Compliance:** Full support for `L402` and legacy `LSAT` HTTP `402 Payment Required` authentication flows.
- **Pluggable Lightning Drivers:**
  - `Mock`: In-memory invoice and preimage simulator for local zero-wallet testing and automated CI.
  - `Lnd`: Direct REST integration with LND nodes for Testnet, Regtest, and Mainnet settlement.
  - `Nwc`: Nostr Wallet Connect (NIP-47) integration for automated client payments and node payouts over Nostr relays.
- **Durable Persistence & Accounting (SQLite WAL Mode):**
  - *Single-Use Payment Authorization*: Strict primary-key deduplication on `payment_hash` prevents replay attacks and double-spending across restarts.
  - *Persistent Session Budgets*: Multi-step sessions with two-phase reservations, spend tracking, and append-only financial accounting ledgers (`sessions`, `session_reservations`, `accounting_entries`).
  - *Durable Inference Lifecycle*: Central `inference_requests` state machine tracking requests from dispatch marker through upstream execution and completion.
  - *Monitored Stream Delivery*: Safe Server-Sent Events (SSE) tracking with drop guards to distinguish natural completion from client disconnects without premature billing.
  - *Conservative Crash Recovery*: Safe automatic rollback for unstarted reservations; ambiguous in-flight compute retained under `reconciliation_required = 1`.
- **OpenAI-Compatible Reverse Proxy:** Transparent routing for `/v1/chat/completions` (JSON and Server-Sent Events streaming) and `/v1/models`.
- **Zero-Logging Privacy Architecture:** Gateway logs exclude request bodies and authorization headers; user prompts and model completions are never written to disk or runtime spans.
- **Nostr Relay Discovery (NIP-89):** Nodes can broadcast availability, supported models, and pricing via Nostr Kind 31990 events.

---

## Current Status & Capabilities

SatsGrid has completed Epics E01–E07, establishing a fully functional permissionless inference ecosystem:

- **PayGuard Accounting Engine:**
  - Strict two-phase reservations enforcing `remaining + reserved + spent = total`.
  - Single-use L402 payment authorization preventing double-spending and replay attacks.
  - Durable inference request lifecycle (`reserved` -> `processing` -> `completed` / `failed` / `stream_interrupted`).
  - Monitored SSE stream delivery with drop guards and atomic transition enforcement.
  - Conservative startup crash recovery retaining ambiguous compute under `reconciliation_required = 1`.
- **Ed25519 Signed Inference Receipts:**
  - Persisted receipts delivered via `x-satsgrid-receipt-id` header or SSE `event: receipt`.
  - Independent CLI verification (`satsgrid receipt verify`) validating provider identity, token usage, and signature.
- **Nostr Decentralized Discovery (Kind 31990):**
  - Persistent provider identity (`.satsgrid_nostr_key`) linked to receipt signing keys.
  - Client-side dynamic routing selecting the lowest-priced online provider (`satsgrid call`).
  - Anti-SSRF endpoint validation rejecting private RFC1918, loopback, link-local, and cloud metadata targets.
- **Accessible & Voice-Assisted Playground:**
  - Speech-to-text voice dictation (STT) guarded against accidental submission.
  - Assistant response read-aloud (TTS) with sentence chunking and credential redaction.
  - Authoritative spoken confirmations gated strictly behind backend payment admission.

---

## Technical Boundaries & Honest Limitations

In alignment with production engineering and audit integrity:
1. **Dynamic Routing Tier**: Dynamic remote provider selection is implemented at the CLI tier (`satsgrid call`). The local Playground and `/v1/chat/completions` route to the node's configured upstream to preserve local L402 and PayGuard non-custodial accounting boundaries.
2. **Provider Health Probing**: Health status currently evaluates passive 24-hour freshness TTLs rather than background active ping daemons.
3. **SSRF Outbound Connection Boundary**: IP validation is performed during discovery; protection against active DNS rebinding or redirect chains at the connection boundary remains an acknowledged future hardening target.
4. **Receipt Cryptographic Scope**: An Ed25519 receipt proves that the provider's key signed the token usage and request metadata; it does not independently prove the semantic honesty of the generated completion or Lightning network finality.
5. **Speech Recognition Support**: Web Speech API dictation is supported in Chromium browsers; desktop Firefox requires external speech engines. Cloud-transcribed audio in certain browsers is governed by browser vendor policies.

---

## Quick Start & Demo Runbook

- **Hackathon Demo Runbook**: Follow step-by-step instructions in [docs/demo-runbook.md](docs/demo-runbook.md) for live node demonstration, Polar Regtest LND payments, receipt verification, and voice playground walkthrough.
- **Developer API Quickstart**: See [docs/quickstart.md](docs/quickstart.md) for cURL, Python, and TypeScript L402 code samples.

### Local Development Quick Start

#### Prerequisites

- [Rust 1.85+](https://rust-lang.org)
- An OpenAI-compatible model runner (such as [Ollama](https://ollama.com) running locally on port 11434).

#### 1. Build SatsGrid

```bash
cargo build --release
```

The resulting binary will be at `target/release/satsgrid`.

#### 2. Configure Your Node

Copy the example configuration:

```bash
cp config/node.example.toml config/node.toml
```

By default, `config/node.toml` binds to `127.0.0.1:8080`, proxies upstream requests to `http://127.0.0.1:11434`, and uses the `mock` Lightning backend for testing without spending real satoshis. For Polar Regtest LND integration, refer to `config/regtest-node.example.toml` and [docs/demo-runbook.md](docs/demo-runbook.md).

#### 3. Start the SatsGrid Node

```bash
./target/release/satsgrid node start --config config/node.toml
```

#### 4. Run an Inference Request (CLI)

In another terminal, send an inference query through the SatsGrid gateway:

```bash
./target/release/satsgrid call \
  --node http://127.0.0.1:8080 \
  --model llama3.2 \
  --prompt "Explain how Bitcoin Lightning enables AI compute access in Africa."
```

When running against the `mock` backend, the CLI client automatically settles the mock invoice challenge and streams the completion response.

---

## Developer API & Integration Quickstart

For developers integrating applications, autonomous agents, or services using cURL, Python, or Node.js, read our comprehensive [Developer API Quickstart Guide](docs/quickstart.md).

It covers:
- HTTP 402 challenge parsing and BOLT11 invoice payment flows.
- Multi-turn Session Budgets and Pay-Per-Request authorization.
- Working examples in cURL, Python (`httpx`), and Node.js (`fetch`).
- Streaming completions via Server-Sent Events (SSE).
- Cryptographic receipt verification and security best practices.

---

## Docker Deployment

To launch SatsGrid alongside an Ollama instance in a single Docker Compose stack:

```bash
cd docker
docker compose up -d
```

Pull a model into the containerized Ollama instance:

```bash
docker exec -it satsgrid-ollama ollama pull llama3.2
```

---

## License & Attribution

SatsGrid is distributed under the terms of the **MIT License**.

This project builds upon the open-source **Infernos** inference engine codebase:
`Copyright (c) 2026 Infernos Contributors`
