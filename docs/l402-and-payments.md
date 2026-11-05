# L402 Payment Protocol & Macaroons Specification

SatsGrid uses the **L402 protocol** (formerly known as LSAT — Lightning Service Authentication Token) to authenticate and meter inference requests permissionlessly over the Bitcoin Lightning Network.

---

## 1. The L402 Protocol Architecture

L402 bridges two core technologies:
1. **Macaroons**: Cryptographic authorization credentials invented by Google Research that support decentralized delegation and client-attenuated caveats without state synchronization.
2. **Lightning Network Preimages**: Cryptographic proofs of payment tied to hashed timelock contracts (HTLCs).

```text
L402 Credential = Macaroon (HMAC-SHA256 signature + Caveats) + Preimage (32-byte secret)
```

---

## 2. HTTP Protocol Handshake

### Step 1: The Challenge (HTTP 402)
When an unauthenticated request arrives at `/v1/chat/completions`, the node generates a Lightning invoice for the request amount, mints an initial macaroon containing the invoice payment hash, and returns an HTTP `402 Payment Required`:

```http
HTTP/1.1 402 Payment Required
WWW-Authenticate: L402 token="<base64_macaroon>", invoice="<bolt11_invoice>"
Content-Type: application/json

{"error": "Payment required"}
```

The header parameters:
- `token`: Base64-encoded serialized macaroon.
- `invoice`: Standard BOLT-11 Lightning invoice string.

> **Compatibility**: SatsGrid also accepts legacy `LSAT` tokens (`WWW-Authenticate: LSAT ...`) for compatibility with older tooling.

### Step 2: Payment Settlement
The caller pays the BOLT-11 invoice via their Lightning wallet. The Lightning Network guarantees that the wallet only releases funds if the node's Lightning backend reveals the 32-byte payment preimage $R$ where:

$$\text{SHA-256}(R) = H$$

### Step 3: Authorization Proof
The caller resubmits the request, including the token and the secret preimage in the `Authorization` header separated by a colon:

```http
Authorization: L402 <base64_macaroon>:<hex_preimage>
```

---

## 3. Cryptographic Verification Algorithm

When the SatsGrid Gate receives the `Authorization` header, it verifies four criteria:

1. **Preimage Validity**:
   $$\text{SHA-256}(\text{preimage}) \stackrel{?}{=} \text{payment\_hash}$$
2. **Macaroon Signature Integrity**:
   The node verifies that the macaroon's signature chain was computed using the node's 32-byte root secret key:
   $$S_0 = \text{HMAC-SHA256}(K_{root}, \text{identifier})$$
   $$S_i = \text{HMAC-SHA256}(S_{i-1}, C_i) \quad \text{for each caveat } C_i$$
3. **Caveat Enforcement**:
   - `time < <expiry_unix_timestamp>`: Current time must be before expiry.
   - `model = <model_name>`: Requested model must match permitted model.
   - `session = <session_uuid>`: Identifies multi-turn session budget state.
   - `budget = <total_sats>`: Declares initial maximum authorized spend.
4. **Backend Settlement**:
   The node verifies via its Lightning backend that the invoice has been marked settled.

---

## 4. Macaroon Caveats Implemented in SatsGrid

| Caveat Format | Description | Verification Logic |
| :--- | :--- | :--- |
| `time < <timestamp>` | Token expiration | `SystemTime::now() < expiration` |
| `model = <id>` | Model restriction | `request.model == id` |
| `session = <uuid>` | Multi-turn session ID | Matches active session in budget tracker |
| `budget = <sats>` | Initial session budget | Upper bound on lifetime session spending |
| `capability = <name>` | Service capability | Restricts action (e.g. `inference`) |

---

## 5. Security Properties

- **Non-Forgeable**: Macaroons cannot be crafted or tampered with without knowledge of the 32-byte root secret.
- **Replay Protection**: Each invoice payment hash is unique.
- **Stateless Verification**: For direct pay-per-request calls, the gate does not require a database to verify token authenticity.

---

## 6. Lightning Backends & Network Support

SatsGrid supports multiple Lightning backend implementations and Bitcoin networks:

### Supported Backends
1. **LND REST (`lnd`)**: Connects directly to an LND node's HTTPS REST API for invoice creation, settlement verification, and automated caller payments. Requires no heavy gRPC dependencies.
2. **Mock Backend (`mock`)**: In-memory payment simulation for testing and zero-wallet offline development.
3. **Nostr Wallet Connect (`nwc`)**: Connects via NWC protocol for remote wallet integration.

### Bitcoin Network Modes
- **Testnet (`testnet`)**: Ideal for testing locally or staging environments. Uses testnet satoshis with zero financial risk. BOLT-11 invoices start with `lntb`.
- **Mainnet (`mainnet`)**: For production deployment. Earns real Bitcoin satoshis for GPU inference. BOLT-11 invoices start with `lnbc`.
- **Regtest / Signet (`regtest`, `signet`)**: Supported for custom local Bitcoin development networks.

