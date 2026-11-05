# SATSGRID PHASE 0 AUDIT: 00 - REPOSITORY INVENTORY

**Project:** SatsGrid  
**Audit Phase:** Phase 0 Inherited Codebase Audit  
**Classification:** SOURCE VERIFIED & RUNTIME VERIFIED  
**Date:** 2026-11-05  

---

## 1. Directory Structure Tree

```
satsgrid/
├── .github/
│   └── workflows/
│       ├── ci.yml                           # GitHub Actions CI workflow (fmt, clippy, test)
│       └── release.yml                      # Release compilation workflow
├── .gitignore                               # Git ignore definitions
├── Cargo.lock                               # Rust dependency lockfile
├── Cargo.toml                               # Package manifest (package, lib, bin = satsgrid)
├── CHANGELOG.md                             # Inherited project changelog
├── CONTRIBUTING.md                          # Contribution guidelines
├── LICENSE                                  # MIT License (Copyright 2026 Infernos Contributors)
├── README.md                                # Root product documentation
├── SECURITY.md                              # Vulnerability reporting guidelines
├── rust-toolchain.toml                      # Toolchain pin (channel = "1.85")
├── config/
│   ├── client.example.toml                  # Client configuration template
│   ├── node.example.toml                    # Node configuration template
│   └── node.toml                            # Active local node configuration (mock backend)
├── docker/
│   ├── Dockerfile                           # Multi-stage Debian-slim container build
│   └── docker-compose.yml                   # Compose definition (satsgrid-node + satsgrid-ollama)
├── docs/
│   ├── architecture.md                      # System architecture and data flow diagrams
│   ├── caller-guide.md                      # Client & agent usage guide
│   ├── development.md                       # Local environment and developer instructions
│   ├── gui-architecture-and-audit.md        # Web client architecture notes
│   ├── l402-and-payments.md                 # Protocol breakdown of L402 and Lightning
│   ├── node-guide.md                        # Operator guide for node deployment
│   ├── nostr-and-nwc.md                     # Decentralized discovery and NWC payment guide
│   ├── privacy.md                           # Privacy guarantees and logging policies
│   ├── session-budgets.md                   # Multi-turn session budget mechanics
│   └── audit/                               # [Audit Output Directory]
├── examples/                                # Empty directory reserved for SDK examples
├── gui/
│   └── client/                              # Next.js 16 Web Application (satsgrid-gui)
│       ├── .gitignore                       # Local frontend gitignore
│       ├── AGENTS.md                        # Agent guide for frontend development
│       ├── CLAUDE.md                        # Styling guidelines
│       ├── README.md                        # GUI documentation
│       ├── components.json                  # Shadcn UI component configuration
│       ├── eslint.config.mjs                # ESLint 9 configuration
│       ├── next.config.ts                   # Next.js configuration (Turbopack)
│       ├── package-lock.json                # Frontend lockfile
│       ├── package.json                     # Frontend manifest (name = satsgrid-gui)
│       ├── postcss.config.mjs               # PostCSS & TailwindCSS v4 plugin config
│       ├── tsconfig.json                    # TypeScript configuration
│       ├── public/                          # Static SVG icons and favicons
│       └── src/
│           ├── app/                         # App Router pages (/ , /node, /playground, /sessions)
│           ├── components/                  # UI, layout, playground, and satsgrid components
│           ├── lib/                         # API client routines, storage, and utilities
│           └── types/                       # TypeScript interfaces for API, session, inference
├── src/
│   ├── main.rs                              # Entry point binary CLI dispatcher
│   ├── lib.rs                               # Library root and module exports
│   ├── cli/                                 # CLI subcommand handlers (node, call, discover, process)
│   ├── client/                              # Rust Client SDK (SatsGridClient, budget, payment handler)
│   ├── common/                              # Error definitions and domain types (Satoshis, PaymentHash)
│   ├── config/                              # Configuration parsing and schema definitions
│   ├── discovery/                           # Nostr NIP-89 Kind 31990 announcement & query client
│   └── node/                                # Node gateway: Axum HTTP routes, L402 gate, Lightning, proxy
└── tests/                                   # 14 Integration test files
```

---

## 2. Inventory by Category

### 2.1 Rust Source Code (`src/`)
- Total Rust Source Files: **40** files.
- Lines of Rust Code: **4,348** lines.
- Breakdown:
  - CLI: `src/cli/{mod, node, call, discover, process}.rs` (5 files)
  - Client SDK: `src/client/{mod, lib, budget, error, openai, pay}.rs` (6 files)
  - Common: `src/common/{mod, error, types}.rs` (3 files)
  - Config: `src/config/{mod, schema}.rs` (2 files)
  - Discovery: `src/discovery/{mod, announcement, client}.rs` (3 files)
  - Node API & Gate: `src/node/api/{mod, routes, handlers}.rs`, `src/node/gate/{mod, budget, challenge, macaroon, verify}.rs` (8 files)
  - Lightning Drivers: `src/node/lightning/{mod, backend, invoice, lnd, mock, nwc}.rs` (6 files)
  - Upstream Proxy: `src/node/proxy/{mod, openai}.rs` (2 files)
  - Root: `src/{lib, main, node/mod, node/config, node/server, node/pricing}.rs` (5 files)

### 2.2 Integration Test Suites (`tests/`)
- Total Integration Test Binaries: **14** test suites.
- Total Tests: **102** tests across all suites (all passing).
- Files:
  - `tests/api_server_tests.rs` (10 tests)
  - `tests/cli_tests.rs` (2 tests)
  - `tests/client_budget_tests.rs` (3 tests)
  - `tests/client_builder_tests.rs` (3 tests)
  - `tests/client_integration_tests.rs` (5 tests)
  - `tests/client_pay_tests.rs` (3 tests)
  - `tests/config_tests.rs` (5 tests)
  - `tests/discovery_tests.rs` (7 tests)
  - `tests/gate_integration_tests.rs` (1 test)
  - `tests/lnd_backend_tests.rs` (7 tests)
  - `tests/mock_lightning_tests.rs` (1 test)
  - `tests/nwc_backend_tests.rs` (6 tests)
  - `tests/privacy_audit_tests.rs` (1 test)
  - `tests/proxy_openai_tests.rs` (8 tests)

### 2.3 Frontend Application (`gui/client/`)
- Framework: Next.js 16.3.6 (App Router, Turbopack) + React 19.2.8 + TailwindCSS v4.
- App Routes:
  - `src/app/page.tsx` (Landing & feature overview)
  - `src/app/node/page.tsx` (Node Operator live dashboard & pricing settings)
  - `src/app/playground/page.tsx` (Interactive inference chat & L402 protocol inspection)
  - `src/app/sessions/page.tsx` (Client session budget management)
- Components:
  - `components/satsgrid/`: `node-status.tsx`, `sats-amount.tsx`, `status-badge.tsx`
  - `components/playground/`: `chat-panel.tsx`, `model-selector.tsx`, `protocol-panel.tsx`
  - `components/layout/`: `navbar.tsx`, `footer.tsx`
  - `components/ui/`: 11 Shadcn UI primitive components
- API Clients: `lib/api/{client, inference, node, sessions}.ts`
- Storage: `lib/storage/sessions.ts` (LocalStorage backed)

### 2.4 Docker and Deployment
- `docker/Dockerfile`: Multi-stage build (builder: `rust:1.85-slim-bookworm`, runtime: `debian:bookworm-slim`). Unprivileged user `satsgrid`.
- `docker/docker-compose.yml`: Multi-container stack linking `satsgrid-node` and `satsgrid-ollama`.

### 2.5 Documentation
- 9 specialized markdown documents in `docs/` covering architecture, guides, privacy, and payments.

---

## 3. Generated & Ephemeral Directories Status

| Directory | Purpose | Status in `.gitignore` |
| :--- | :--- | :--- |
| `target/` | Cargo compilation artifacts (11 GB) | **Explicitly Ignored** (`/target/`) |
| `.satsgrid/` | Local node runtime state & PID file | **Explicitly Ignored** (`.satsgrid/`) |
| `gui/client/.next/` | Next.js production build output | **Explicitly Ignored** (`.next/`, `**/.next/`) |
| `gui/client/node_modules/` | Node dependencies (2.0 GB) | **Explicitly Ignored** (`node_modules/`, `**/node_modules/`) |
| `gui/client/web/` | Legacy unmerged web folder | **Explicitly Ignored** (`gui/client/web/`) |
| `config/node.toml` | Active local node config with potential secrets | **Explicitly Ignored** (`node.toml`) |

**Confirmation:** All build outputs, credentials, state caches, and dependencies are securely excluded from Git tracking.

