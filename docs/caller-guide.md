# SatsGrid Caller & Autonomous Agent Guide

This guide explains how humans and autonomous agents consume inference from an SatsGrid node without accounts, API keys, or subscriptions, paying per request or per session with Satoshis.

---

## 1. Discovering Live Nodes (`satsgrid discover`)

Before sending inference requests, autonomous agents and callers can dynamically discover active SatsGrid nodes across Nostr relays without relying on a centralized index:

```bash
# Discover all active SatsGrid nodes on default relays
satsgrid discover

# Find nodes offering a specific model
satsgrid discover --model llama3.2

# Output machine-readable JSON for automated agent pipelines
satsgrid discover --model llama3.2 --json
```

Example JSON output for agent routing:
```json
[
  {
    "service": "satsgrid",
    "name": "Alpha Node",
    "node_url": "https://alpha.satsgrid.org",
    "models": ["llama3.2", "mistral"],
    "pricing_sats": 10,
    "network": "mainnet",
    "version": "0.1.0",
    "pubkey": "3bf0c63fcb93463407af97b5e09188382d8..."
  }
]
```

---

## 2. Using the Command-Line Interface (CLI)

The easiest way to query an SatsGrid node directly is via the `satsgrid call` command.

### Basic Pay-per-Request Query
```bash
cargo run -- call \
  --node http://127.0.0.1:8080 \
  --model llama3.2 \
  --prompt "What is the difference between L402 and an API key?"
```

Output:
```text
SatsGrid
──────────────────────────────
Node:      http://127.0.0.1:8080
Model:     llama3.2
Budget:    100 sats
Payment:   MOCK Lightning

✓ Session created
✓ L402 authorization established
✓ Inference request accepted

Response
──────────────────────────────
An API key is a centralized identity credential tied to an account and credit card,
whereas L402 is an open, self-sovereign payment authorization protocol that pairs
cryptographic macaroons with Lightning Network payment preimages.

──────────────────────────────
Charged:    10 sats
Remaining:  90 sats
```

---

## 3. Using the Rust SDK (`satsgrid::client`)

For developers integrating autonomous agents or microservices, the built-in Rust client handles the full L402 handshake, budget tracking, and streaming automatically.

```rust
use satsgrid::client::builder::ClientBuilder;
use satsgrid::client::budget::ClientBudgetTracker;
use satsgrid::client::openai::{ChatCompletionRequest, ChatMessage};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Setup payment provider and budget tracker
    let budget_tracker = Arc::new(ClientBudgetTracker::new(500)); // 500 Sats max

    // 2. Build the client
    let client = SatsGridClient::builder()
        .node_url("http://127.0.0.1:8080")
        .payment_provider(my_lightning_provider)
        .build()?
        .with_budget(budget_tracker.clone());

    // 3. Create session (pre-fund budget)
    client.create_session(500).await?;

    // 4. Send chat request
    let request = ChatCompletionRequest {
        model: "llama3.2".to_string(),
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "Draft a smart contract audit checklist.".to_string(),
        }],
        stream: None,
    };

    let response = client.chat(&request).await?;
    println!("Completion: {}", response.choices[0].message.content);
    println!("Remaining budget: {} sats", budget_tracker.remaining());

    Ok(())
}
```

---

## 3. Integrating with Standard OpenAI Tooling & LangChain

SatsGrid exposes standard OpenAI endpoints. Any HTTP client or agent framework can interact with an SatsGrid node using the standard L402 challenge flow:

### Raw HTTP Flow with `curl`

#### Step 1: Send Request without Credentials
```bash
curl -i -X POST http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model": "llama3.2", "messages": [{"role": "user", "content": "Hello!"}]}'
```

Response:
```http
HTTP/1.1 402 Payment Required
WWW-Authenticate: L402 token="AGF2M...base64...", invoice="lnbc100n1..."
Content-Type: application/json

{"error": "Payment required"}
```

#### Step 2: Pay the Invoice
Pay the BOLT-11 invoice using your Lightning wallet (e.g. Alby, Strike, CashApp, `lncli payinvoice`). The wallet returns a 32-byte hex payment preimage (proof of payment).

#### Step 3: Resend Request with Proof
```bash
curl -X POST http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: L402 AGF2M...base64...:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" \
  -d '{"model": "llama3.2", "messages": [{"role": "user", "content": "Hello!"}]}'
```

Response:
```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "id": "chatcmpl-...",
  "object": "chat.completion",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "Hello! How can I help you today?"
      },
      "finish_reason": "stop"
    }
  ]
}
```

---

## 4. Querying Available Models
To query models dynamically served by the node:
```bash
curl http://127.0.0.1:8080/v1/models
```
Response:
```json
{
  "object": "list",
  "data": [
    {
      "id": "llama3.2",
      "object": "model",
      "created": 1700000000,
      "owned_by": "infernos-node"
    }
  ]
}
```
