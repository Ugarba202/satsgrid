# Phase 0 Audit: Documentation Accuracy and Consistency Audit

**Status:** SOURCE VERIFIED  
**Target Directory:** `docs/` (9 markdown guides)  

---

## 1. Documentation Inventory & Scope

The repository provides extensive technical documentation detailing architecture, operational setup, caller workflows, protocol specifications, and security model:

1. `docs/architecture.md`: System design, component boundaries, and L402 flow.
2. `docs/node-guide.md`: Node operator onboarding, hardware requirements, and configuration.
3. `docs/caller-guide.md`: Client workflows, CLI invocation, and Rust SDK integration.
4. `docs/l402-and-payments.md`: L402 specification, HMAC-SHA256 caveats, and Lightning backends.
5. `docs/session-budgets.md`: Multi-turn session budgets, token debiting, and caveats.
6. `docs/nostr-and-nwc.md`: NIP-47 wallet payments and NIP-89 / Kind 31990 node discovery.
7. `docs/privacy.md`: Ephemeral memory, zero prompt logging guarantees, and telemetry.
8. `docs/development.md`: Contributor prerequisites, cargo commands, and quality gates.
9. `docs/gui-architecture-and-audit.md`: Next.js web application architecture and API client.

---

## 2. Documentation Accuracy Matrix

| Document | Verified Accurate Concepts | Drift / Stale Artifacts Identified |
|---|---|---|
| `architecture.md` | L402 handshake, reverse proxy concept, Axum routing, Macaroon caveats. | Minor: Mentions `infernos` in historical context. |
| `node-guide.md` | TOML structure, Ollama setup, ports, environment variables. | **Stale CLI references:** References `./target/release/infernos`, `infernos node start`, `infernos node announce`, and `docker exec -it infernos-ollama`. Needs alignment to `satsgrid`. |
| `caller-guide.md` | Header formats, L402 challenge flow, JSON response shapes. | **Stale CLI & SDK references:** References `infernos discover`, `infernos call`, and Rust imports `use infernos::client::...`. Must be `satsgrid`. |
| `l402-and-payments.md` | Caveat strings, HMAC signing, preimage validation. | Accurately describes protocol. |
| `session-budgets.md` | Caveat format (`budget = <sats>`, `session = <id>`), debit lifecycle. | Accurately describes in-memory budget state. |
| `nostr-and-nwc.md` | NIP-47 protocol, NIP-89 announce tags (`31990`). | **Stale discovery tags:** Mentions `#t="infernos"`, `["d", "infernos-node"]`, and `infernos discover`. The active code in `src/nostr/discovery.rs` now filters on tag `"satsgrid"`. |
| `privacy.md` | Zero prompt logging, in-memory state, tracing setup. | Accurate to code implementation in `src/proxy/handler.rs`. |
| `development.md` | Rust toolchain, test suites, cargo targets. | Accurate. |
| `gui-architecture-and-audit.md` | Next.js App Router, LocalStorage keys, client endpoints. | References `infernos-node` and `infernos-logo.svg` in diagrams. |

---

## 3. Discrepancies Between Documentation and Implementation

1. **CLI Binary Command Names:**
   - *Documentation states:* `infernos node start`, `infernos call`, `infernos discover`.
   - *Implementation actual:* The compiled binary is `satsgrid` (`satsgrid node start`, `satsgrid call`, `satsgrid discover`).
2. **Nostr Discovery Tags:**
   - *Documentation states:* Nodes advertise tag `#t="infernos"` with identifier `d="infernos-node"`.
   - *Implementation actual:* `src/nostr/discovery.rs` advertises `#t="satsgrid"` and queries relay filters for tag `"satsgrid"`.
3. **Rust SDK Module Paths:**
   - *Documentation states:* `use infernos::client::builder::ClientBuilder;`.
   - *Implementation actual:* The crate is named `satsgrid` (`use satsgrid::client::builder::ClientBuilder;`).
4. **Docker Container Names:**
   - *Documentation states:* `infernos-ollama`.
   - *Implementation actual:* `docker/docker-compose.yml` specifies `container_name: satsgrid-ollama` and `container_name: satsgrid-node`.

---

## 4. Preservation of Historical Attribution

The root `LICENSE` file preserves the copyright and attribution:
```
Copyright (c) 2026 Infernos Contributors
```
Documentation will update active operational commands to `satsgrid` while leaving legal copyright notices intact.
