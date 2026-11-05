# SatsGrid Architecture & System Design

**Permissionless Open-Model Inference Paid in Sats via L402**  
Machine Money Track · BOSS Battle 2026

---

## 1. System Overview

SatsGrid turns any machine running open-weight AI models into a self-sovereign, paid inference endpoint. It sits as a lightweight, auditable reverse proxy and payment gate directly in front of an OpenAI-compatible inference engine (such as Ollama, vLLM, or llama.cpp).

```mermaid
flowchart TB
    subgraph Caller["Caller (Client)"]
        Client[Autonomous Agent / User]
        Wallet[Lightning Wallet]
    end

    subgraph Node["SatsGrid Node"]
        Gate["L402 Payment Gate<br/>(Axum + Tokio)"]
        Pricing["Pricing Engine"]
        BudgetMgr["Session Budget Manager"]
        MacaroonSvc["Macaroon Service"]
        Proxy["OpenAI Proxy"]
    end

    subgraph Upstream["Upstream Engine & Hardware"]
        Engine["Inference Engine<br/>(Ollama / vLLM)"]
        GPU["Local Hardware<br/>(GPU / VRAM)"]
    end

    subgraph LN["Lightning Network"]
        LND["Lightning Node / NWC"]
    end

    Client -->|1. HTTP Request| Gate
    Gate --> Pricing
    Pricing -->|Calculate Cost| Gate
    Gate -->|2. HTTP 402 + Invoice| Client
    Client -->|3. Pay Invoice| Wallet
    Wallet -->|Settlement| LND
    Client -->|4. Retry with L402 Token:Preimage| Gate
    Gate --> MacaroonSvc
    Gate --> BudgetMgr
    Gate -->|5. Forward Validated Request| Proxy
    Proxy -->|Forward Request| Engine
    Engine --> GPU
    Engine -->|6. Completion / SSE Stream| Proxy
    Proxy --> Gate
    Gate -->|7. HTTP 200 + Response| Client
```

---

## 2. Core Components

### 2.1 L402 Payment Gate (`src/node/gate/`)
The Gate intercepts incoming HTTP requests to protected endpoints (`/v1/chat/completions`) and enforces standard L402 payment validation:
- **`challenge.rs`**: Generates and parses standard HTTP `402 Payment Required` headers:
  `WWW-Authenticate: L402 token="<macaroon>", invoice="<bolt11>"`
- **`macaroon.rs`**: Issues and verifies HMAC-SHA256 cryptographically chained macaroons with optional caveats (`time`, `model`, `session`, `budget`).
- **`verify.rs`**: Parses `Authorization: L402 <macaroon>:<preimage>` headers, verifies `SHA-256(preimage) == payment_hash`, checks backend settlement, and validates caveats.
- **`budget.rs`**: Thread-safe, in-memory session balance manager (`SessionBudgetManager`) enforcing atomic CAS balance debits and exhaustion checks.

### 2.2 Pricing Engine (`src/node/pricing.rs`)
Calculates inference invoice costs dynamically:
- Base flat rate per request (`default_price_sats` / `sats_per_request`).
- Per-token pricing (`sats_per_prompt_token`, `sats_per_completion_token`).
- Built-in heuristic token estimator for OpenAI chat completion payloads.

### 2.3 OpenAI Proxy (`src/node/proxy/`)
- Interacts with upstream inference engines (e.g. `http://127.0.0.1:11434`).
- Supports both standard JSON request/response forwarding and real-time Server-Sent Events (SSE) streaming (`stream: true`).
- Performs dynamic upstream model discovery (`GET /v1/models`) with fallback to configured default models.

### 2.4 Lightning Backend Abstraction (`src/node/lightning/`)
- Defines an asynchronous `LightningBackend` trait:
  - `create_invoice(amount: Satoshis, description: &str) -> Result<Invoice>`
  - `is_invoice_settled(payment_hash: &PaymentHash) -> Result<bool>`
  - `pay_invoice(bolt11: &str) -> Result<Preimage>`
- Ships with an in-memory `MockLightningBackend` for deterministic, zero-network local development and automated testing.
- Extensible to LND gRPC and Nostr Wallet Connect (NWC).

---

## 3. End-to-End Sequence Flows

### 3.1 Direct Pay-per-Request Flow

```mermaid
sequenceDiagram
    autonumber
    actor Caller as Caller (Agent / User)
    participant Gate as SatsGrid Gate
    participant LN as Lightning Backend
    participant Engine as Upstream (Ollama)

    Caller->>Gate: POST /v1/chat/completions (No Auth)
    Gate->>LN: create_invoice(cost)
    LN-->>Gate: Invoice (payment_hash, bolt11)
    Gate->>Gate: mint_macaroon(payment_hash)
    Gate-->>Caller: 402 Payment Required<br/>WWW-Authenticate: L402 token="...", invoice="..."

    Note over Caller,LN: Caller pays Lightning invoice off-chain
    Caller->>LN: Pay BOLT-11 invoice
    LN-->>Caller: Payment Preimage

    Caller->>Gate: POST /v1/chat/completions<br/>Authorization: L402 <macaroon>:<preimage>
    Gate->>Gate: Verify SHA-256(preimage) == payment_hash
    Gate->>Gate: Verify Macaroon HMAC signature
    Gate->>LN: Verify invoice is settled
    Gate->>Engine: Forward sanitized request
    Engine-->>Gate: Completion response
    Gate-->>Caller: 200 OK + Completion JSON
```

### 3.2 Pre-Funded Session Budget Flow

```mermaid
sequenceDiagram
    autonumber
    actor Caller as Autonomous Agent
    participant Gate as SatsGrid Gate
    participant Budget as Budget Manager
    participant Engine as Upstream (Ollama)

    Caller->>Gate: POST /v1/session/new {"budget_sats": 500}
    Gate-->>Caller: 402 Payment Required (Invoice for 500 sats)
    Note over Caller: Agent settles 500 sats invoice
    Caller->>Gate: POST /v1/chat/completions (Session Macaroon + Preimage)
    Gate->>Budget: debit_session(session_id, cost_sats)
    alt Budget Available
        Budget-->>Gate: Approved (Remaining: 490 sats)
        Gate->>Engine: Forward request
        Engine-->>Gate: Completion
        Gate-->>Caller: 200 OK + Completion + X-SatsGrid-Remaining-Budget-Sats: 490
    else Budget Exhausted
        Budget-->>Gate: Rejected (Insufficient balance)
        Gate-->>Caller: 402 Payment Required (Budget Exhausted)
    end
```

---

## 4. Design & Security Principles

1. **Self-Sovereign**: Node operators control their hardware and earn Sats directly.
2. **Permissionless**: No accounts, usernames, credit cards, or centralized API keys.
3. **Privacy by Default**: Zero prompt logging. Prompts and completions are processed in memory and never written to disk or logs.
4. **Economic Safety**: Session budget caveats strictly constrain agent spending limits.
