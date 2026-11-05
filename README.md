# SatsGrid

**Africa-first AI compute access powered by Bitcoin Lightning.**

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
|  - Atomic In-Memory & Session Spending Budget Enforcement   |
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

### Key Capabilities Inherited from Infernos

- **L402 Protocol Compliance:** Full support for `L402` and legacy `LSAT` HTTP `402 Payment Required` authentication flows.
- **Pluggable Lightning Drivers:**
  - `Mock`: In-memory invoice and preimage simulator for local zero-wallet testing and automated CI.
  - `Lnd`: Direct REST integration with LND nodes for Testnet, Regtest, and Mainnet settlement.
  - `Nwc`: Nostr Wallet Connect (NIP-47) integration for automated client payments and node payouts over Nostr relays.
- **Dual-Mode Billing:**
  - *Pay-Per-Request*: Issue on-demand invoices for single completions.
  - *Session Budgets*: Pre-fund multi-turn chat sessions with lock-free Compare-And-Swap (CAS) atomic budget debiting.
- **OpenAI-Compatible Reverse Proxy:** Transparent routing for `/v1/chat/completions` (JSON and Server-Sent Events streaming) and `/v1/models`.
- **Zero-Logging Privacy Architecture:** Gateway logs exclude request bodies and authorization headers; user prompts and model completions are never written to disk or runtime spans.
- **Nostr Relay Discovery (NIP-89):** Nodes can broadcast availability, supported models, and pricing via Nostr Kind 31990 events.

---

## Current Limitations & Roadmap

While SatsGrid provides a robust protocol foundation, several advanced capabilities are planned for upcoming milestones:

- **Settlement Timing & PayGuard:** The current engine settles payments prior to forwarding inference requests to the upstream model engine. If an upstream engine crashes or times out, payments are not automatically refunded. The upcoming **PayGuard** capability will introduce conditional hold-invoice settlement to guarantee inference integrity.
- **In-Memory Session Volatility:** Active session balances currently reside in process memory. A node restart clears active session balances. Persistent storage (SQLite) is planned.
- **Provider Marketplace Routing:** Discovered Nostr nodes currently function as an open bulletin board; dynamic automated multi-node routing and GPU attestation will be built in the next development phase.

---

## Quick Start

### Prerequisites

- [Rust 1.85+](https://rust-lang.org)
- An OpenAI-compatible model runner (such as [Ollama](https://ollama.com) running locally on port 11434).

### 1. Build SatsGrid

```bash
cargo build --release
```

The resulting binary will be at `target/release/satsgrid`.

### 2. Configure Your Node

Copy the example configuration:

```bash
cp config/node.example.toml config/node.toml
```

By default, `config/node.toml` binds to `127.0.0.1:8080`, proxies upstream requests to `http://127.0.0.1:11434`, and uses the `mock` Lightning backend for testing without spending real satoshis.

### 3. Start the SatsGrid Node

```bash
./target/release/satsgrid node start --config config/node.toml
```

### 4. Run an Inference Request (CLI)

In another terminal, send an inference query through the SatsGrid gateway:

```bash
./target/release/satsgrid call \
  --node http://127.0.0.1:8080 \
  --model llama3.2 \
  --prompt "Explain how Bitcoin Lightning enables AI compute access in Africa."
```

When running against the `mock` backend, the CLI client automatically settles the mock invoice challenge and streams the completion response.

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
