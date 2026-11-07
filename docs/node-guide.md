
# SatsGrid Node Operator Guide

This guide walks operators through turning a machine with local open-weight models into a self-sovereign inference endpoint that earns Satoshis over the Lightning Network.

---

## 1. Hardware & System Prerequisites

SatsGrid is lightweight (~15MB compiled binary) and connects to any OpenAI-compatible inference engine running locally.

### Recommended System Specifications
- **Operating System**: Linux (Ubuntu 22.04+ recommended), macOS, or Windows (WSL2 / native).
- **RAM / VRAM Requirements**:
  - Small models (`llama3.2:1b`, `llama3.2:3b`, `qwen2.5:3b`): 4GB – 8GB VRAM or 16GB system RAM.
  - Mid models (`llama3.1:8b`, `mistral:7b`): 8GB – 16GB VRAM (NVIDIA RTX 3060/4060 or Apple Silicon M-series).
  - Large models (`command-r`, `llama3.3:70b`): Multi-GPU or high-memory unified systems.
- **Inference Runtime**: [Ollama](https://ollama.ai) (recommended for ease of use) or [vLLM](https://github.com/vllm-project/vllm).

---

## 2. Quick Setup with Ollama

### Step 1: Install and Launch Ollama
```bash
# Install Ollama (Linux/macOS)
curl -fsSL https://ollama.com/install.sh | sh

# Pull desired models
ollama pull llama3.2
ollama pull mistral
```
By default, Ollama listens on `http://127.0.0.1:11434`.

---

## 3. Configuring the SatsGrid Node

Copy the example configuration file:
```bash
cp config/node.example.toml config/node.toml
```

### Configuration Reference (`config/node.toml`)

```toml
[server]
# Host address to bind the SatsGrid L402 Gate
host = "0.0.0.0"
# Port for the public API
port = 8080

[upstream]
# Address of the local inference engine
url = "http://127.0.0.1:11434"

[pricing]
# Default base price per request in Satoshis
default_price_sats = 10

# Optional fine-grained per-token pricing (in satoshis)
# sats_per_prompt_token = 0
# sats_per_completion_token = 0

[lightning]
# Lightning backend: "lnd", "mock", or "nwc"
backend = "lnd"

# Bitcoin network: "testnet" (for testing locally with testnet sats) or "mainnet" (live operations)
network = "testnet"

# LND REST configuration
# lnd_rest_host = "https://127.0.0.1:8080"
# lnd_macaroon_path = "~/.lnd/data/chain/bitcoin/testnet/admin.macaroon"
# lnd_tls_cert_path = "~/.lnd/tls.cert"
```

---

## 3.1 Setting Up LND for Testnet vs Mainnet

SatsGrid communicates directly with LND using its built-in REST API, requiring no heavy gRPC dependencies.

### Option A: Testing Locally with Bitcoin Testnet
Testnet allows you to test real Lightning payments and session budgeting without risking real capital.

1. **Start your LND node on testnet**:
   Ensure LND's `lnd.conf` has `bitcoin.testnet=1` and `restlisten=0.0.0.0:8080`.
2. **Configure SatsGrid for Testnet**:
   ```toml
   [lightning]
   backend = "lnd"
   network = "testnet"
   lnd_rest_host = "https://127.0.0.1:8080"
   lnd_macaroon_path = "~/.lnd/data/chain/bitcoin/testnet/admin.macaroon"
   lnd_tls_cert_path = "~/.lnd/tls.cert"
   ```
   *Note: Invoices generated on Testnet begin with the prefix `lntb`.*

### Option B: Live Operation on Bitcoin Mainnet (Earning Real Sats)
When you are ready to earn real Bitcoin Satoshis for serving inference:

1. **Ensure your Mainnet LND node is fully synced and funded with channels**:
   Ensure LND's `lnd.conf` has `bitcoin.mainnet=1` and `restlisten=0.0.0.0:8080`.
2. **Configure SatsGrid for Mainnet**:
   ```toml
   [lightning]
   backend = "lnd"
   network = "mainnet"
   lnd_rest_host = "https://127.0.0.1:8080"
   lnd_macaroon_path = "~/.lnd/data/chain/bitcoin/mainnet/admin.macaroon"
   lnd_tls_cert_path = "~/.lnd/tls.cert"
   ```
   *Note: Invoices generated on Mainnet begin with the standard prefix `lnbc`.*

### Option C: Zero-Wallet Local Mock (Offline Development)
For fast offline development and running the test suite without any Bitcoin node running:
```toml
[lightning]
backend = "mock"
```

### Security Best Practices for Macaroons
- **Least-Privilege Macaroon**: For production, you can use `invoice.macaroon` instead of `admin.macaroon` if you only need the node to generate invoices and verify settlement.
- **TLS Verification**: Point `lnd_tls_cert_path` to your LND `tls.cert` so all HTTP REST calls are encrypted and certificate-verified.

---

## 4. Operating the Node Lifecycle

The SatsGrid CLI provides full process lifecycle controls:

### Start the Node
```bash
cargo run -- node start --config config/node.toml

# Or using the built release binary:
./target/release/satsgrid node start --config config/node.toml
```

Output:
```text
Starting SatsGrid node...
✓ Configuration loaded from config/node.toml
✓ Lightning backend: lnd (testnet)
✓ Upstream: http://127.0.0.1:11434
✓ Server listening on 0.0.0.0:8080

SatsGrid node is running.
```
*(If configured for mock, this will display `✓ Lightning backend: mock`)*

### Check Node Status & Health
```bash
cargo run -- node status
```

Output:
```text
SatsGrid Node
─────────────
Status:       RUNNING
PID:          42891
Endpoint:     0.0.0.0:8080
Upstream:     http://127.0.0.1:11434
Price:        10 sats/request
Lightning:    LND (testnet)
```

### Stop the Node
Gracefully shut down the running node:
```bash
cargo run -- node stop
```

---

## 5. Running with Docker Compose

To run both the SatsGrid Node and an Ollama container in a single command:
```bash
docker compose -f docker/docker-compose.yml up -d
```

To pull a model into the containerized Ollama instance:
```bash
docker exec -it satsgrid-ollama ollama pull llama3.2
```
Your node is now accessible on port `8080`!

---

## 6. Decentralized Nostr Announcements & Node Discovery

SatsGrid nodes can announce their availability to public Nostr relays without depending on any centralized indexing server.

### Configuring Nostr Discovery (`config/node.toml`)
```toml
[discovery]
# Automatically announce to Nostr on node start
enabled = true

# Relays to broadcast availability to
relays = [
    "wss://relay.damus.io",
    "wss://nos.lol"
]

# Display name for your node
node_name = "My GPU Node"

# Publicly reachable endpoint URL
public_url = "https://node.satsgrid.org"

# Optional: Nostr private key (nsec) to maintain a persistent identity across announcements
# nsec = "nsec1..."
```

### Announcing via CLI

1. **Broadcast on Startup**:
   ```bash
   satsgrid node start --config config/node.toml --announce
   ```

2. **Broadcast On Demand**:
   ```bash
   # Use configuration file
   satsgrid node announce --config config/node.toml

   # Override relays or private key
   satsgrid node announce --config config/node.toml --relay wss://relay.damus.io
   ```

