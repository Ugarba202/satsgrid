# Privacy Model & Zero Prompt Logging Guarantees

SatsGrid treats user and agent privacy as a **non-negotiable design constraint**. Unlike centralized AI providers that retain, inspect, and train on user prompts, SatsGrid is built on the principle of **zero prompt logging by default**.

---

## 1. Core Privacy Guarantees

| Guarantee | Mechanism |
| :--- | :--- |
| **Zero Prompt Logging** | Request and response bodies are processed entirely in-memory and are never written to server logs, tracing spans, or persistent storage. |
| **No Account Tracking** | Requests are authenticated purely by cryptographic proofs of payment (preimages). No user accounts, passwords, or emails exist. |
| **Ephemeral Session State** | Session budgets reside strictly in volatile memory. No prompt history is saved. |
| **Minimal Metadata** | Only timestamps, HTTP status codes, and latency are logged for basic node health monitoring. |

---

## 2. Technical Enforcement in the Codebase

### 2.1 Tracing Configuration
The HTTP server utilizes `tower_http::trace::TraceLayer` configured with header and body logging explicitly disabled:

```rust
// src/node/server.rs
let app = create_routes(state).layer(
    TraceLayer::new_for_http()
        .make_span_with(DefaultMakeSpan::new().include_headers(false)),
);
```

### 2.2 Automated Privacy Audit Test
To prevent accidental regressions, the repository includes an automated privacy audit test in [`tests/privacy_audit_tests.rs`](../tests/privacy_audit_tests.rs):

```rust
#[tokio::test]
async fn test_privacy_audit_prompt_not_logged() {
    let captured_logs = Arc::new(Mutex::new(Vec::new()));
    // Set up in-memory tracing subscriber...
    
    let canary_prompt = "CANARY_TOKEN_SECRET_987654321_PROMPT_DO_NOT_LOG";
    // Send request through the API server...
    
    // Assert canary prompt NEVER appears in any captured log span
    assert!(!all_logs.contains(canary_prompt));
}
```

Any pull request that inadvertently logs request bodies will immediately fail continuous integration (CI).

---

## 3. Threat Model

### 3.1 Honest-But-Curious Operator
- **Threat**: A node operator inspects standard server logs or disk dumps to see what callers are asking the LLM.
- **Mitigation**: SatsGrid does not log prompts or completions anywhere. Disk logs only record connection metrics.

### 3.2 Network Eavesdropping
- **Threat**: An on-path network observer monitors unencrypted HTTP traffic between caller and node.
- **Mitigation**: Production nodes should terminate TLS (via reverse proxies such as Caddy or Nginx) or bind to a Tor Onion Service (`.onion`).

### 3.3 Malicious Caller (Denial-of-Service)
- **Threat**: A caller floods the node with compute-heavy generation prompts.
- **Mitigation**: Every inference request requires a confirmed Lightning payment before upstream compute is dispatched. Attempted spam incurs financial cost for the attacker.

---

## 4. Running the Privacy Audit Locally

You can verify the privacy guarantees on your own machine at any time:
```bash
cargo test --test privacy_audit_tests
```
Expected output:
```text
running 1 test
test test_privacy_audit_prompt_not_logged ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; finished in 0.12s
```
