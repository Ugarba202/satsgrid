# Phase 0 Audit: Frontend Application Architecture & Audit

**Status:** SOURCE VERIFIED & RUNTIME VERIFIED  
**Package:** `satsgrid-gui` (`gui/client`)  
**Framework:** Next.js 16.3.6 (App Router), React 19.2.8, TailwindCSS 4.2.1, TypeScript 5.9.3  

---

## 1. Executive Summary

The SatsGrid frontend is a dedicated Next.js client application designed to interface with local or remote SatsGrid nodes. It provides two operational views:
1. **Operator Console (`/node`):** A monitoring and administration portal for node hosts, displaying live server status, hosted Ollama models, request throughput, sats earned, and configuration parameters.
2. **Caller Portal (`/`, `/playground`, `/sessions`):** A client-facing suite allowing users to negotiate L402 sessions, fund spend budgets via Bitcoin Lightning (WebLN, Nostr Wallet Connect NIP-47, or manual preimage entry), inspect cryptographic caveats, and stream OpenAI-compatible chat completions with real-time sat deduction feedback.

All branding and internal keys have been successfully migrated to `satsgrid` (e.g. `satsgrid_sessions_v1`, `satsgrid_admin_token`, `satsgrid_nwc_uri`, `components/satsgrid/`). Build verification via `npm run build` succeeds completely (exit code 0).

---

## 2. Directory Structure & Component Inventory

```
gui/client/src/
├── app/
│   ├── layout.tsx              # Root HTML wrapper with fonts and globals
│   ├── globals.css             # Tailwind 4 theme variables (colors, borders, fonts)
│   ├── page.tsx                # Landing page explaining SatsGrid mechanics
│   ├── favicon.ico
│   ├── node/
│   │   └── page.tsx            # Node Dashboard (health, models, admin stats/config)
│   ├── playground/
│   │   └── page.tsx            # Interactive chat playground & L402 challenge flow
│   └── sessions/
│       └── page.tsx            # Session budget manager & capability history
├── components/
│   ├── layout/
│   │   ├── navbar.tsx          # Top nav, network switcher, WebLN Connect button
│   │   └── footer.tsx          # Bottom branding and links
│   ├── playground/
│   │   ├── chat-panel.tsx      # Chat stream display, prompt input, invoice dialogs
│   │   ├── protocol-panel.tsx  # L402 challenge breakdown, session budget sliders
│   │   └── model-selector.tsx  # Model dropdown dynamically querying /v1/models
│   ├── satsgrid/
│   │   ├── node-status.tsx     # Node health ping indicator
│   │   ├── sats-amount.tsx     # Formatted satoshi counter component
│   │   └── status-badge.tsx    # CVA status badge component
│   └── ui/                     # Reusable UI primitives (button, card, dialog, progress, etc.)
├── lib/
│   ├── api/
│   │   ├── client.ts           # fetchWithL402 wrapper catching HTTP 402 and parsing headers
│   │   ├── inference.ts        # SSE streaming client with chunk and budget callbacks
│   │   ├── node.ts             # REST client for /health, /v1/models, /v1/node/*
│   │   └── sessions.ts         # Session creation API caller (/v1/session/new)
│   ├── storage/
│   │   └── sessions.ts         # LocalStorage persistence for sessions and active state
│   └── utils.ts                # Tailwind clsx/twMerge utility
└── types/
    ├── api.ts                  # L402Challenge, L402Error definitions
    ├── inference.ts            # ChatCompletionRequest, Message, StreamChunk definitions
    └── session.ts              # SessionStatus, SessionData definitions
```

---

## 3. Application Pages & Route Audit

### 3.1 Landing Page (`/` - `src/app/page.tsx`)
- **Visuals & Copy:** Clearly communicates the SatsGrid identity: *"Open Models. Paid in Sats. Permissionless open-model inference powered by Lightning + cryptographic authorization."*
- **Architecture Highlights:**
  - Explains the 4-step execution lifecycle: **Request → Pay → Prove → Infer**.
  - Highlights dual user personas: Operator vs. Caller.
  - Zero presence of deprecated project names. External documentation link points to GitHub repository.

### 3.2 Node Dashboard (`/node` - `src/app/node/page.tsx`)
- **Capabilities:**
  - Unauthenticated fetch of `/health` (service name, status, version) and `/v1/models` (active models reported by Ollama).
  - Authenticated fetch of `/v1/node/stats` and `/v1/node/config` using an operator Bearer token (`satsgrid_admin_token` stored in `localStorage`).
  - Real-time display of total requests processed, active RAM sessions, total satoshis earned, host binding, Lightning backend (`mock`, `lnd`, or `nwc`), and flat/token pricing.
  - Informational panels detailing L402 gate enforcement, WebLN/NWC settlement, and token budgeting.

### 3.3 Chat Playground (`/playground` - `src/app/playground/page.tsx`)
- **Capabilities:**
  - Allows callers to select target inference models (`llama3.2`, etc.) and set session budgets (e.g., 50, 100, 250, 500 sats).
  - Triggers session creation via `POST /v1/session/new`, intercepting the `402 Payment Required` challenge.
  - Unpacks the BOLT11 invoice and macaroon caveats (`session`, `budget`, `capability`).
  - Supports 3 payment resolution pathways:
    1. **WebLN / Bitcoin Connect:** One-click settlement via browser extensions (Alby, etc.).
    2. **Nostr Wallet Connect (NWC):** Automated payment via remote NWC pairing strings (`nostr+walletconnect://...`).
    3. **Manual Preimage Input / Mock Pay:** Direct 64-character hex preimage entry or direct testing against `/internal/mock/pay`.
  - Initiates streaming inference to `POST /v1/chat/completions` with header `Authorization: L402 <macaroon>:<preimage>`.
  - Parses real-time HTTP response headers `x-satsgrid-charged-sats` and `x-satsgrid-remaining-budget-sats` to decrement callers' live balances during token streaming.

### 3.4 Sessions Manager (`/sessions` - `src/app/sessions/page.tsx`)
- **Capabilities:**
  - Surfaces all historical sessions stored in browser `localStorage`.
  - Tracks total sat capability purchased vs. total satoshis consumed.
  - Provides visual progress bars of remaining session balances.
  - Allows callers to switch active sessions, resume historical conversations, or purge stale tokens.

---

## 4. API Client & Protocol Layer

### 4.1 L402 Challenge Handling (`src/lib/api/client.ts`)
```typescript
export async function fetchWithL402(url: string, options: RequestInit = {}) {
  const res = await fetch(url, options);

  if (res.status === 402) {
    const authHeader = res.headers.get("www-authenticate");
    if (authHeader && authHeader.startsWith("L402")) {
      const tokenMatch = authHeader.match(/token="([^"]+)"/);
      const invoiceMatch = authHeader.match(/invoice="([^"]+)"/);

      if (tokenMatch && invoiceMatch) {
        const macaroon = tokenMatch[1];
        const invoice = invoiceMatch[1];
        throw new L402Error({
          macaroon,
          invoice,
          parsedCaveats: parseMacaroonCaveats(macaroon)
        });
      }
    }
  }
...
```
- **Caveat Parsing:** Safely decodes base64-encoded macaroons in-browser (`atob`), parsing first-party caveats (`session = <id>`, `budget = <sats>`, `capability = <cap>`) into structured JavaScript objects.

### 4.2 Streaming Inference Client (`src/lib/api/inference.ts`)
- Utilizes standard Web Streams API (`ReadableStreamDefaultReader` + `TextDecoder`).
- Consumes Server-Sent Events (`data: {"choices":[{"delta":{"content":"..."}}]}`).
- Reads custom SatsGrid debit headers:
  - `x-satsgrid-charged-sats`
  - `x-satsgrid-remaining-budget-sats`
- Signals parent React components via callbacks for live rendering and local balance updates.

---

## 5. Storage & Persistence (`src/lib/storage/sessions.ts`)

The frontend manages state entirely client-side using `window.localStorage`:
- `satsgrid_sessions_v1`: Array of `StoredSession` records containing `id`, `model`, `budget_sats`, `remaining_sats`, `macaroon`, `preimage`, and chat `messages`.
- `satsgrid_active_session_id`: ID of the currently selected session.
- `satsgrid_admin_token`: Operator authentication token used on `/node`.
- `satsgrid_nwc_uri`: Persisted Nostr Wallet Connect connection string.

**Critical Architectural Finding:**
Because the backend node does not persist session balances to disk, if the SatsGrid backend restarts, the frontend's stored macaroons will still have remaining budgets locally, but the backend will return `402 Payment Required` or `401 Unauthorized` (Session not found in RAM). The frontend handles this by displaying an authorization error prompt.

---

## 6. Build and Linter Status

- **Build Verification (`npm run build`):**
  - Next.js 16.3.6 Turbopack compiler compiles all routes successfully.
  - Outputs static HTML and RSC manifests for `/`, `/node`, `/playground`, and `/sessions`.
  - Exit code: `0` (Success).
- **Linter Output (`npm run lint`):**
  - Completed with exit code `0`.
  - Reports style warnings and strict TypeScript lint rules from `@next/eslint-plugin-next` and `typescript-eslint` (typical for Next.js 16 strict mode on legacy components), but no breaking syntax or module resolution errors.
