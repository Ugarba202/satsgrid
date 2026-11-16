# SatsGrid Demo Runbook
## Dakar Bitcoin Hack 2026 — Machine Money & AI Track

This runbook documents the exact, verified demonstration flow for **SatsGrid**, connecting Bitcoin Lightning payments with open-weight AI inference through standard L402 HTTP authentication and Ed25519 cryptographic receipts.

---

### Critical Architectural Distinctions & Scope

1. **Routing Boundary**:
   - **CLI (`satsgrid call`)**: Performs client-side dynamic provider discovery via Nostr (Kind 31990), evaluates pricing, and targets the selected provider node.
   - **Playground & Backend REST API (`/v1/chat/completions`)**: Exclusively routes through the node's locally configured inference upstream (`config.upstream.url`, e.g., local Ollama / vLLM) to preserve non-custodial L402 and PayGuard accounting boundaries.
2. **Local vs Remote Network Scope**:
   - Advertising `http://127.0.0.1:8080` in a Nostr announcement is appropriate for local devnet testing, but loopback addresses are rejected by remote clients enforcing anti-SSRF policies.
   - A Nostr announcement proves that a node signed an announcement event; it does not guarantee remote network reachability.
3. **Environment Scope**:
   - The verified demonstration uses **Bitcoin Regtest** (with real LND REST integration) or the in-memory **Mock backend** for deterministic automated testing. It does not use Bitcoin Mainnet.

---

### Prerequisites & Verified Services

Before executing the demo, verify that the required local host services are active:

1. **AI Compute Backend (Ollama)**:
   - Check if Ollama is running and serving `llama3.2:latest`:
     ```bash
     curl -s http://127.0.0.1:11434/api/tags
     ```
   - *Note: If Ollama is running as a system service on port 11434, you do not need to run `ollama serve` manually.*

2. **Lightning Backend (Polar LND Regtest)**:
   - Node Bob (Provider Gateway): REST listening on `https://127.0.0.1:8082` (or configured port in `config/regtest-node.example.toml`).
   - Provider Macaroon: `.satsgrid/regtest/bob_admin.macaroon` (or Polar volume path).
   - Provider TLS Cert: `.satsgrid/regtest/bob_tls.cert` (or Polar volume path).
   - Payer Wallet: Polar Node Alice or external WebLN wallet with active channel balance to Bob.

---

### Step 1: Start the SatsGrid Node Server

Start the node server using the Regtest configuration:

```bash
cargo run -- node start --config config/regtest-node.example.toml
```

**Expected Startup Output:**
```text
Starting SatsGrid node...
✓ Configuration loaded from config/regtest-node.example.toml
✓ Lightning backend: Lnd (Regtest)
✓ Upstream: http://127.0.0.1:11434
✓ Server listening on 127.0.0.1:8080
SatsGrid node is running.
```

**Verify Health Endpoint:**
```bash
curl -s http://127.0.0.1:8080/health
```
*Expected JSON:*
```json
{"lightning":{"backend":"lnd_rest","network":"regtest"},"service":"satsgrid-node","status":"ok","version":"0.1.0"}
```

---

### Step 2: Start the Next.js Frontend Interface

In a second terminal, launch the web application:

```bash
cd gui/client
npm run dev
```

**Expected Output:**
```text
▲ Next.js 16.3.6 (Turbopack)
- Local: http://localhost:3000
✓ Ready in 1.2s
```

Open `http://localhost:3000/playground` in your browser.

---

### Step 3: Broadcast Node Availability to Nostr Relays

Broadcast the node's availability to Nostr relays (Kind 31990):

```bash
cargo run -- node announce --config config/regtest-node.example.toml
```

**Expected Output:**
```text
Broadcasting SatsGrid node announcement to Nostr relays: ["wss://relay.damus.io", "wss://nos.lol"]
✓ Node announcement successfully published!
  Event ID:    <nostr_event_id_hex>
  Public Key:  <node_nostr_public_key_hex>
  Endpoint:    http://127.0.0.1:8080
  Models:      ["llama3.2:latest"]
  Price:       10 sats
```

*Note: The node's Nostr key is persisted in `.satsgrid-regtest/.satsgrid_nostr_key` (or `<data_dir>/.satsgrid_nostr_key`), ensuring the same public key is preserved across restarts.*

---

### Step 4: Discover Providers via CLI & Model Explorer

1. **CLI Discovery**:
   ```bash
   cargo run -- discover --timeout-secs 3
   ```
2. **Provider & Model Explorer Web UI**:
   - Navigate to `http://localhost:3000/explorer`.
   - Inspect registered models, per-model pricing overrides, and discovered provider cards.

---

### Step 5: Execute Pay-Per-Request Inference (L402 Handshake)

#### A. Initial Request without Authorization
```bash
curl -i -s -X POST http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "llama3.2:latest",
    "messages": [{"role": "user", "content": "Explain permissionless AI in 1 sentence."}]
  }'
```

**Expected Response (`HTTP 402 Payment Required`):**
```http
HTTP/1.1 402 Payment Required
Content-Type: application/json
WWW-Authenticate: L402 token="<macaroon_base64>", invoice="lnbcrt100n1..."

{"error":"Payment required","invoice":"lnbcrt100n1...","token":"<macaroon_base64>"}
```

#### B. Pay Invoice with Polar LND Payer Wallet
1. Extract the `invoice` BOLT11 string from the 402 response.
2. In Polar (or via Alice's CLI), pay the invoice:
   ```bash
   # Example paying from Alice node via polar/lncli:
   lncli --network regtest --rpcserver 127.0.0.1:10001 payinvoice <invoice_bolt11>
   ```
3. Capture the returned 64-character hex payment preimage (e.g. `<payment_preimage_hex>`). Keep it local; never commit or leak credentials.

#### C. Retry Request with L402 Credentials
```bash
curl -i -s -X POST http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: L402 <macaroon_base64>:<payment_preimage_hex>" \
  -d '{
    "model": "llama3.2:latest",
    "messages": [{"role": "user", "content": "Explain permissionless AI in 1 sentence."}]
  }'
```

**Expected Response (`HTTP 200 OK`):**
```http
HTTP/1.1 200 OK
Content-Type: application/json
x-satsgrid-charged-sats: 10
x-satsgrid-receipt-id: <receipt_uuid>
...

{
  "choices": [
    {
      "message": {
        "role": "assistant",
        "content": "<generated_model_completion>"
      }
    }
  ]
}
```

---

### Step 6: Retrieve & Independently Verify Signed Receipt

1. **Retrieve the Receipt via REST**:
   ```bash
   curl -s http://127.0.0.1:8080/v1/receipts/<receipt_uuid> > /tmp/receipt.json
   ```

2. **Verify Ed25519 Cryptographic Signature with CLI**:
   ```bash
   cargo run -- receipt verify --file /tmp/receipt.json
   ```
   **Expected Verification Output:**
   ```text
   ✓ Cryptographic signature is valid!
     Receipt ID:     <receipt_uuid>
     Request ID:     <request_uuid>
     Key ID:         satsgrid-node-...
     Public Key:     <node_ed25519_public_key_hex>
     Payment Hash:   <payment_hash_hex>
     Model:          llama3.2:latest
     Tokens:         Prompt: <count>, Completion: <count>
     Charge:         10 sats
     Timestamp:      <timestamp_iso8601>
   ```

3. **Verify in Playground UI**:
   - Beneath the assistant response in the browser, click the **Signed Receipt** pill badge.
   - Inspect the receipt verification status, execution metadata, and CLI verification syntax in the modal.

---

### Step 7: Demonstrate Playground Accessibility & Voice

1. **Voice Input (STT)**:
   - In a Chromium browser, click the microphone button in the chat input bar.
   - Dictate a prompt. The transcribed text populates the textarea.
   - **Safety Invariant**: Voice input *never* auto-submits. The user must explicitly press Send to authorize payment.
2. **Read-Aloud (TTS)**:
   - Click the "Read Aloud" speaker button on the assistant response.
   - Speech synthesis reads the response, chunked by sentences. Sensitive credentials (tokens, preimages) are automatically redacted.
3. **Spoken Confirmations**:
   - In the Protocol Activity header, check "Spoken Confirmations".
   - Confirm that payment confirmation announcements fire *only* after backend admission (`onAuthorized`).
