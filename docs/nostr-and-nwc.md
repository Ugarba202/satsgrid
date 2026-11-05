# Nostr & Nostr Wallet Connect (NIP-47) in SatsGrid

This document explains the role of the Nostr protocol and NIP-47 (Nostr Wallet Connect — NWC) in SatsGrid, covering architecture, caller automated payments, operator node receiving, and decentralized node discovery.

---

## 1. Why Nostr in SatsGrid?

SatsGrid provides permissionless, self-sovereign AI inference paid in satoshis. In production environments, callers (autonomous agents or terminal users) and operators face two primary friction points:

1. **Lightning Infrastructure Burden**: Running a full LND or CLN node requires synchronized chain data, open payment channels, TLS certificates, and macaroon management.
2. **Centralized Discovery**: Finding available inference nodes typically relies on centralized directories or web scrapers.

Nostr solves both challenges:
- **NIP-47 (Nostr Wallet Connect)**: Uses Nostr relays as an end-to-end encrypted messaging pipe between SatsGrid and remote Lightning wallets (such as Alby Hub, Umbrel, Mutiny, Phoenix, or Voltage).
- **Nostr Relays for Node Discovery**: Allows SatsGrid nodes to publish cryptographically signed announcements of their available models, pricing, and URLs.

---

## 2. NIP-47 Protocol Architecture

```mermaid
sequenceDiagram
    autonumber
    participant Agent as Autonomous Agent / CLI
    participant Gate as SatsGrid L402 Gate
    participant Relay as Nostr Relay (wss://...)
    participant Wallet as Caller Remote Wallet (Alby / Umbrel)

    Agent->>Gate: POST /v1/chat/completions (Request)
    Gate-->>Agent: 402 Payment Required (L402 Macaroon + BOLT11 Invoice)
    Agent->>Relay: Kind 23194 (Encrypted NIP-47 pay_invoice Request)
    Relay->>Wallet: Deliver Request
    Wallet->>Wallet: Settle BOLT11 over Lightning Network
    Wallet->>Relay: Kind 23195 (Encrypted NIP-47 Response + Preimage)
    Relay-->>Agent: Deliver Preimage
    Agent->>Gate: POST /v1/chat/completions + Authorization: L402 <macaroon>:<preimage>
    Gate->>Gate: Cryptographically verify preimage and macaroon
    Gate-->>Agent: 200 OK + AI Completion Stream
```

---

## 3. Autonomous Caller / Agent Configuration

Callers can settle L402 challenges completely automatically without manual copy-pasting or holding node secrets on disk.

### Option A: CLI Flag `--payer-nwc-uri`
```bash
infernos call \
  --node http://127.0.0.1:8080 \
  --model llama3.2 \
  --prompt "Explain Bitcoin in one sentence" \
  --payer-nwc-uri "nostr+walletconnect://<wallet-pubkey>?relay=wss%3A%2F%2Frelay.damus.io&secret=<client-secret>"
```

### Option B: Environment Variable `INFERNOS_PAYER_NWC_URI`
For autonomous background agents, configure the environment variable:
```bash
export INFERNOS_PAYER_NWC_URI="nostr+walletconnect://<wallet-pubkey>?relay=wss%3A%2F%2Frelay.damus.io&secret=<client-secret>"
infernos call --model llama3.2 --prompt "Analyze system metrics"
```

When configured, the SatsGrid CLI banner will output:
```text
SatsGrid
──────────────────────────────
Node:      http://127.0.0.1:8080
Model:     llama3.2
Budget:    100 sats
Payment:   NWC (Automated)
```

---

## 4. Operator Node Configuration

Operators can configure their SatsGrid node to receive Lightning payments into any remote NWC-compatible wallet instead of running a local LND instance.

In `config/node.toml`:
```toml
[server]
host = "0.0.0.0"
port = 8080

[lightning]
backend = "nwc"
nwc_uri = "nostr+walletconnect://<wallet-pubkey>?relay=wss%3A%2F%2Frelay.damus.io&secret=<secret>"
```

Or via environment variable:
```bash
export INFERNOS_NWC_URI="nostr+walletconnect://..."
infernos start
```

### Supported NIP-47 Methods:
- `make_invoice`: Generates BOLT11 invoices with custom amounts (in millisatoshis) and descriptions.
- `lookup_invoice`: Verifies payment settlement via payment hash.
- `pay_invoice`: Settles outgoing invoices (used by caller client payment provider).

---

## 5. Decentralized Node Discovery via Nostr

SatsGrid eliminates the need for centralized directories or registries. Nodes can broadcast their service availability and capabilities directly over Nostr relays using parameterized addressable events (NIP-89 / Kind `31990`).

```mermaid
sequenceDiagram
    autonumber
    participant Node as SatsGrid Node (Operator)
    participant Relay as Nostr Relays (wss://...)
    participant Agent as Autonomous Agent / User

    Note over Node: Node starts or runs announce
    Node->>Relay: Publish Kind 31990 Event (Models, Pricing, Endpoint)
    Note over Agent: Discover nodes by model or network
    Agent->>Relay: REQ Kind 31990 + #t="infernos" + #m="llama3.2"
    Relay-->>Agent: Matching Node Announcements
    Agent->>Node: Directly call inference endpoint over L402
```

### 5.1 Event Specification (Kind 31990)

| Field / Tag | Specification | Description |
| :--- | :--- | :--- |
| **Kind** | `31990` | Parameterized replaceable application handler / service announcement |
| **`d` Tag** | `["d", "infernos-node"]` | Unique identifier tag |
| **`t` Tag** | `["t", "infernos"]`, `["t", "ai-inference"]` | Discoverability hashtags |
| **`endpoint` Tag** | `["endpoint", "https://node.example.com"]` | Public HTTP(S) URL of the SatsGrid node |
| **`pricing` Tag** | `["pricing", "10"]` | Default base price in satoshis |
| **`network` Tag** | `["network", "testnet"]` or `["network", "mainnet"]` | Bitcoin network for Lightning settlement |
| **`m` Tags** | `["m", "llama3.2"]`, `["m", "mistral"]` | AI models served by this node |
| **Content** | JSON Payload | Serialized `NodeAnnouncement` schema with version and metadata |

### 5.2 Discovering Nodes via CLI

Callers and agents can discover live nodes querying one or more Nostr relays:

```bash
# Discover all active SatsGrid nodes across default relays
infernos discover

# Filter by a specific model (e.g. llama3.2)
infernos discover --model llama3.2

# Query custom relays with custom timeout
infernos discover --relay wss://relay.damus.io --relay wss://nos.lol --timeout-secs 10

# Output machine-readable JSON (ideal for autonomous agent scripts)
infernos discover --model llama3.2 --json
```

Sample output:
```text
Discovered 2 active SatsGrid node(s):

NODE NAME            ENDPOINT                       MODELS                   PRICE (SATS)    NETWORK   
---------------------------------------------------------------------------------------------------------
Llama Alpha          https://alpha.infernos.org     llama3.2, mistral        10              mainnet   
GPU Hub Regtest      http://127.0.0.1:8080          llama3.2, deepseek-r1    5               regtest   
---------------------------------------------------------------------------------------------------------
```

### 5.3 Publishing Node Announcements

#### Option A: Automatic Broadcast on Node Startup
Add `--announce` when starting the node:
```bash
infernos node start --config config/node.toml --announce
```

Or enable it permanently in `config/node.toml`:
```toml
[discovery]
enabled = true
relays = ["wss://relay.damus.io", "wss://nos.lol"]
node_name = "My GPU Cluster"
public_url = "https://node.example.com"
```

#### Option B: Standalone CLI Announce
Broadcast node availability on demand:
```bash
# Announce using node.toml configuration
infernos node announce --config config/node.toml

# Announce to specific relays with custom nsec key
infernos node announce --config config/node.toml --relay wss://relay.damus.io --nsec nsec1...
```

---

## 6. Security & Privacy Guarantees

1. **End-to-End Encryption (NIP-04 / NIP-44)**: All NIP-47 requests and responses passing through Nostr relays are encrypted using Diffie-Hellman shared secrets. Relay operators cannot read invoices, amounts, or preimages.
2. **Spend Limits & Granular Permissions**: NWC connections can be scoped in the user's wallet with spending allowances (e.g., maximum 500 sats per day), preventing rogue spending.
3. **No Private Keys Exposed**: The SatsGrid client only holds a temporary connection secret, never the master wallet seed or Lightning private keys.
4. **Zero Prompt Leakage**: Prompts and AI completions are sent strictly over HTTPS directly between caller and node; they are never published to Nostr relays.
5. **Cryptographic Identity**: Node announcements are signed with standard Schnorr signatures (NIP-01). Callers can verify that announcements originate authentically from the node operator's public key.

