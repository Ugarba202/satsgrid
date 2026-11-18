# SatsGrid

\<p align="center">   \<strong>Permissionless AI Inference. Paid in Sats. Accessible by Design.\</strong> \</p>

\<p align="center">   \<a href="[https://youtu.be/eDXZndAu0OQ](https://youtu.be/eDXZndAu0OQ)">Watch the SatsGrid Demo\</a>   ·   \<a href="#getting-started">Getting Started\</a>   ·   \<a href="#architecture">Architecture\</a>   ·   \<a href="#developer-integration">Developer Integration\</a> \</p>

---

## What is SatsGrid?

SatsGrid is an open, permissionless AI inference gateway that connects developers with independent AI compute providers through **Bitcoin Lightning payments, the L402 protocol, and Nostr-based provider discovery**.

It enables developers to access open-weight AI models without depending exclusively on conventional API subscriptions, centralized billing systems, or international payment cards.

Instead of requiring a recurring subscription or a traditional account-based billing relationship, SatsGrid introduces a payment-enabled inference workflow where access can be authorized through Lightning payments.

Compute providers can expose their available models and pricing, while developers can discover providers and pay for inference using satoshis.

SatsGrid combines three important technologies:

- **AI inference:** OpenAI-compatible access to open-weight models through inference runtimes such as Ollama and vLLM.
- **Bitcoin Lightning:** Fast, low-value payments suitable for individual inference requests and session-based access.
- **Nostr:** A decentralized communication and discovery layer that allows providers to announce their availability, supported models, and pricing.

At the center of the system is **PayGuard**, SatsGrid's request-level authorization, reservation, accounting, and inference lifecycle management engine.

Our vision is to make AI compute more accessible, open, and economically interoperable, with an Africa-first perspective.

---

## The Problem

AI is becoming an important part of software development, research, education, and business. However, accessing AI infrastructure remains difficult for many developers, particularly those outside established technology markets.

We identify three interconnected problems.

### 1. Restricted access to AI compute

Many AI services depend on centralized platforms with account requirements, subscription plans, usage restrictions, and geographic limitations.

Developers in emerging markets can face additional barriers, including:

- Limited access to international payment cards.
- Foreign currency restrictions and conversion costs.
- Difficulty subscribing to international developer services.
- Limited access to affordable inference infrastructure.
- Dependence on a small number of centralized API providers.

These barriers can prevent capable developers, students, researchers, and startups from accessing the compute they need.

### 2. Traditional AI billing is not designed for open participation

Most inference APIs rely on conventional account-based billing.

Developers typically need to register, maintain an account balance or subscription, configure payment methods, and operate within the provider's billing ecosystem.

This creates friction for applications that need flexible, programmatic, pay-per-use access.

Bitcoin Lightning provides an alternative payment rail that supports small, fast payments without requiring every application to depend on a conventional card-based billing system.

### 3. Independent compute providers lack a common payment and discovery layer

Individuals, organizations, and infrastructure operators may have available GPU or other compute resources that could serve AI workloads.

However, making that infrastructure accessible to external developers requires more than hosting a model.

Providers need a way to:

- Advertise available models.
- Publish pricing and service information.
- Receive inference requests.
- Authorize access based on payment.
- Account for usage.
- Provide developers with verifiable request records.

Without a common discovery and payment mechanism, independent compute providers can remain disconnected from developers who need their services.

---

## Our Solution

SatsGrid provides a payment-enabled gateway that brings developers and independent AI compute providers together.

It combines OpenAI-compatible inference interfaces, Lightning-based authorization, decentralized provider discovery, and request-level accounting.

### For developers

Developers can:

- Access AI models through familiar OpenAI-compatible interfaces.
- Pay for inference using Bitcoin Lightning.
- Use L402 payment challenges to obtain request authorization.
- Create and manage multi-request inference sessions.
- Inspect usage and payment-related accounting records.
- Verify provider-signed inference receipts.
- Discover advertised providers through Nostr.
- Use the SatsGrid CLI to select an available provider based on advertised pricing.

### For compute providers

Providers can:

- Run a SatsGrid node.
- Connect an OpenAI-compatible inference backend.
- Configure Lightning payment infrastructure.
- Advertise supported models and pricing through Nostr.
- Accept payment-authorized inference requests.
- Maintain request lifecycle and accounting records.
- Produce signed inference receipts.

The result is a foundation for an open AI compute marketplace where access and payment do not need to depend on a single centralized billing platform.

---

## How SatsGrid Works

SatsGrid brings together provider discovery, payment authorization, inference execution, and accounting.

The following describes the main workflow.

### 1. A provider makes compute available

A compute provider operates a SatsGrid node and connects it to an OpenAI-compatible inference runtime.

For example, the provider may run an open-weight model using Ollama or vLLM.

The provider configures the node's supported models, pricing, Lightning backend, and public identity.

Provider information can be announced through Nostr using the SatsGrid discovery mechanism.

### 2. A developer discovers a provider

A developer can discover available providers and their advertised models and pricing through Nostr.

The SatsGrid CLI supports dynamic provider selection.

In the implemented CLI workflow, the client discovers providers, evaluates advertised pricing, and selects an appropriate available provider.

This makes it possible to build applications that are not permanently tied to a single compute endpoint.

### 3. The developer requests inference

The developer submits an inference request through the SatsGrid interface.

SatsGrid checks whether the request has valid payment authorization.

If authorization is missing, the node returns an HTTP `402 Payment Required` challenge containing the information needed to complete the L402 payment flow.

### 4. Lightning payment authorizes access

The developer's Lightning wallet pays the invoice associated with the challenge.

After payment, the client obtains the payment proof required by the L402 flow.

SatsGrid verifies the authorization information before allowing the protected request to proceed.

The payment authorization is single-use, preventing the same payment proof from being reused to authorize multiple independent requests.

### 5. PayGuard manages request admission and accounting

Before execution, PayGuard coordinates the request's reservation and authorization lifecycle.

It tracks:

- Available session capacity.
- Reserved amounts.
- Amounts charged for completed inference.
- Request execution state.
- Failed or interrupted requests.
- Ambiguous outcomes requiring reconciliation.

This is important because an inference request is not always a simple transaction.

A client can disconnect, a model runtime can fail, or a process can restart while computation is in progress.

PayGuard maintains durable request state so the node can distinguish completed, failed, interrupted, and uncertain executions.

### 6. The model executes the request

Once the request is admitted, SatsGrid forwards it to the configured OpenAI-compatible upstream inference backend.

The backend generates the response.

SatsGrid supports normal JSON responses and Server-Sent Events (SSE) streaming for chat completions.

### 7. SatsGrid produces an inference receipt

After a request completes, SatsGrid can generate a provider-signed inference receipt.

The receipt contains relevant request metadata and usage information.

The receipt is signed using Ed25519 and can be independently checked using the receipt verification functionality.

This gives developers a portable record of the provider's signed statement about the inference request.

---

## Architecture

SatsGrid is designed as a modular gateway between developers, payment infrastructure, provider discovery, and AI inference backends.

```text
                     DEVELOPERS & APPLICATIONS
                  CLI / Playground / API Clients
                               |
                               |
                      HTTP / OpenAI API
                               |
                               v
                 +---------------------------+
                 |       SATSGRID NODE       |
                 |                           |
                 |  OpenAI-Compatible API    |
                 |  L402 Authorization       |
                 |  PayGuard Engine          |
                 |  Session Management       |
                 |  Request Lifecycle        |
                 |  Receipt Generation       |
                 +---------------------------+
                       |               |
                       |               |
                       v               v
              +----------------+  +------------------+
              | BITCOIN        |  | AI INFERENCE     |
              | LIGHTNING      |  | BACKEND          |
              |                |  |                  |
              | LND            |  | Ollama           |
              | NWC             |  | vLLM             |
              | Mock            |  | OpenAI-compatible|
              +----------------+  +------------------+
                       |
                       |
                       v
                PAYMENT AUTHORIZATION

                +----------------------+
                |   NOSTR DISCOVERY    |
                |                      |
                | Provider Announcements|
                | Supported Models     |
                | Advertised Pricing   |
                +----------------------+
```

### Core components

| Component          | Responsibility                                                            |
| ------------------ | ------------------------------------------------------------------------- |
| SatsGrid Node      | Main gateway between developers, payments, and inference backends         |
| PayGuard           | Request authorization, reservations, accounting, and lifecycle management |
| Lightning Adapter  | Interfaces with Lightning payment infrastructure                          |
| L402 Middleware    | Handles HTTP 402 challenges and payment authorization                     |
| Inference Adapter  | Forwards requests to OpenAI-compatible model backends                     |
| Nostr Discovery    | Publishes and discovers provider information                              |
| Receipt System     | Generates and verifies Ed25519-signed inference receipts                  |
| Playground         | Provides a browser-based interface for interacting with SatsGrid          |
| Operator Dashboard | Exposes node health, model information, and accounting information        |
| CLI                | Provides command-line inference and provider discovery workflows          |

---

## PayGuard: The Request Authorization and Accounting Engine

PayGuard is one of the central components of SatsGrid.

It manages the relationship between payment authorization and inference execution.

A payment alone should not mean that an arbitrary number of inference requests can be executed. Each admitted request must be accounted for, and the system must handle failures without incorrectly treating uncertain executions as completed.

### Two-phase reservation model

PayGuard uses reservations to account for funds associated with in-flight requests.

The session accounting invariant is:

```text
remaining + reserved + spent = total
```

Where:

- `total` is the session's original capability.
- `remaining` is the amount available for new requests.
- `reserved` is the amount assigned to in-flight requests.
- `spent` is the amount charged for completed inference.

This helps prevent concurrent requests from consuming more than the available session capacity.

### Single-use payment authorization

L402 payment authorization is protected against replay through strict deduplication using the payment hash.

A payment proof that has already been consumed cannot simply be reused to authorize another request.

### Durable request lifecycle

Each inference request is tracked through a persistent lifecycle.

The system records request admission, execution, completion, failure, interruption, and reconciliation states.

This makes accounting more reliable than simply charging whenever a response is returned.

### Crash recovery

A process restart can occur while inference is being executed.

SatsGrid handles this conservatively:

- Reservations belonging to requests that never started can be rolled back safely.
- Requests that may have reached the upstream inference backend are treated as ambiguous.
- Ambiguous requests are marked for reconciliation instead of being automatically declared successful or refunded.

This avoids silently treating uncertain compute execution as a confirmed failure.

### Streaming safety

SatsGrid supports Server-Sent Events for streaming inference responses.

PayGuard monitors stream delivery and uses drop guards to distinguish normal completion from client disconnection.

This is important because a disconnected client does not necessarily mean that the upstream model stopped executing.

---

## Bitcoin Lightning and L402

SatsGrid uses Bitcoin Lightning as its payment rail and L402 as its HTTP payment authorization mechanism.

L402 combines an HTTP `402 Payment Required` challenge with payment and authorization credentials.

The general flow is:

```text
Developer Client
      |
      | 1. Inference request
      v
SatsGrid Gateway
      |
      | 2. HTTP 402 Payment Required
      |    Invoice + Authorization Challenge
      v
Developer Lightning Wallet
      |
      | 3. Pay Lightning invoice
      v
Lightning Network
      |
      | 4. Payment proof / preimage
      v
SatsGrid Gateway
      |
      | 5. Verify L402 authorization
      v
PayGuard
      |
      | 6. Admit and account for request
      v
AI Inference Backend
      |
      | 7. Generate response
      v
Developer Client
```

### Lightning backend support

SatsGrid provides a pluggable Lightning backend architecture.

| Backend | Purpose                                                                            |
| ------- | ---------------------------------------------------------------------------------- |
| Mock    | Simulates invoice and payment behavior for local development and automated testing |
| LND     | Integrates with Lightning Network Daemon for Lightning payments                    |
| NWC     | Provides an integration path through Nostr Wallet Connect (NIP-47)                 |

The Mock backend allows developers to test the payment authorization flow without spending real satoshis.

The LND integration supports development and demonstration environments such as Polar Regtest, as well as configured Lightning environments.

---

## Nostr-Based Provider Discovery

A permissionless compute network needs a way for providers to make their services discoverable.

SatsGrid uses Nostr as its decentralized provider discovery layer.

Providers can publish information describing:

- Provider identity.
- Available inference models.
- Advertised pricing.
- Service endpoint information.
- Availability metadata.

SatsGrid uses Nostr Kind `31990` events for its provider announcement mechanism.

The provider identity is persistent and is linked to the receipt-signing identity.

### Client-side provider selection

The SatsGrid CLI supports dynamic provider selection.

The `satsgrid call` workflow can discover providers through Nostr and select an available provider based on advertised pricing.

This provides a foundation for clients to interact with multiple independent compute providers rather than being permanently tied to one endpoint.

**Implementation distinction:** Dynamic remote provider selection is implemented in the CLI discovery workflow. The Playground and the standard `/v1/chat/completions` API currently use the node's configured upstream inference backend. They do not dynamically route each request across Nostr providers.

### Provider endpoint safety

SatsGrid validates discovered provider endpoints and rejects private RFC1918, loopback, link-local, and cloud metadata targets during discovery validation.

Additional connection-boundary hardening against DNS rebinding and redirect-chain behavior remains a future security improvement.

---

## Signed Inference Receipts

SatsGrid generates Ed25519-signed inference receipts to provide a portable record of completed inference requests.

Receipts can include relevant information such as:

- Receipt identifier.
- Provider identity.
- Request metadata.
- Model information.
- Token usage information.
- Accounting-related information.
- Cryptographic signature.

Receipts are made available through the `x-satsgrid-receipt-id` response header or an SSE `event: receipt` event.

### Why receipts matter

In a distributed compute environment, developers need more than an inference response.

They need a way to retain and verify a provider's signed statement about the request.

SatsGrid's receipt system provides a foundation for:

- Request audit trails.
- Provider accountability.
- Usage verification.
- Independent signature verification.
- Future integrations with external accounting and verification systems.

### Receipt verification

SatsGrid supports receipt verification through its CLI and public verification interface.

The verification process checks the cryptographic signature and can compare the signing identity against an expected provider key.

A valid signature establishes that the corresponding signing key signed the receipt.

It does **not** independently prove that the model generated an honest response or that a Lightning payment reached final settlement.

That distinction is important for any trustworthy distributed inference system.

---

## Developer Experience

SatsGrid aims to make integration straightforward for developers who already use OpenAI-compatible APIs.

### OpenAI-compatible API

SatsGrid exposes:

```text
/v1/chat/completions
/v1/models
```

The chat completion endpoint supports:

- JSON request and response handling.
- Server-Sent Events streaming.
- L402 payment authorization.
- Request-level accounting.
- Session-based authorization.

Existing applications using compatible API clients can integrate with SatsGrid by changing their API endpoint and authentication/payment handling.

### Command-line interface

The SatsGrid CLI provides a way to interact with the node without building a separate application.

Example:

```bash
satsgrid call \
  --node http://127.0.0.1:8080 \
  --model llama3.2 \
  --prompt "Explain how Bitcoin Lightning can enable access to AI compute."
```

The CLI also supports provider discovery and dynamic provider selection through the `satsgrid call` workflow.

### Playground

SatsGrid includes a browser-based Playground for interacting with configured AI models.

It provides:

- Chat-based inference.
- Model selection.
- Cost estimation.
- Session interaction.
- L402 challenge and authorization visibility.
- Signed receipt access.
- Voice input where supported by the browser.
- Assistant response read-aloud.

The Playground provides an accessible way to explore the inference experience without first writing an API integration.

**Note:** The Playground currently uses the node's configured upstream backend rather than dynamically selecting a remote Nostr provider for each request.

---

## Session-Based Inference

Not every application is best served by paying for every interaction independently.

SatsGrid supports multi-request sessions with a defined spending capability.

A session can authorize multiple inference requests within its available budget.

PayGuard tracks:

- Original session capability.
- Remaining amount.
- Reserved amount.
- Spent amount.
- Associated inference requests.

This provides a foundation for applications such as:

- Conversational AI agents.
- Multi-step AI workflows.
- Developer tools.
- Autonomous applications.
- AI-powered services that need controlled spending limits.

The reservation model ensures that concurrent requests are accounted for against the same session capacity.

---

## Operator Experience

SatsGrid also provides an Operator Dashboard for monitoring the node and its inference activity.

The dashboard exposes operational and accounting information, including:

- Node health.
- Hosted models.
- Active sessions.
- Inference request outcomes.
- Settled-revenue accounting totals.
- Active reservations.
- Lightning invoice states.
- Recent inference activity.

Operators can inspect the models available through their configured inference backend and understand how requests are being processed.

The dashboard's revenue figure represents recorded charges from completed inference records. It should not be interpreted as independent proof of Lightning network settlement.

Similarly, active reservations represent funds allocated to in-flight requests, not an unused session balance.

---

## Privacy and Security

Security and accounting integrity are important design considerations in SatsGrid.

### Privacy-conscious logging

The gateway is designed to avoid logging request bodies and authorization headers.

User prompts and model completions are not intentionally written to persistent application logs or runtime tracing spans.

Authorization-sensitive headers are sanitized from logs.

### Payment replay protection

Single-use payment authorization and payment-hash deduplication help prevent replay and double authorization.

### Durable accounting

The request lifecycle and accounting state are persisted so that the system can recover conservatively after process restarts.

### Provider endpoint validation

Nostr-discovered provider endpoints undergo validation to reject private, loopback, link-local, and cloud metadata targets.

### Cryptographic receipts

Ed25519 signatures allow clients to verify that a receipt was signed by the corresponding provider key.

### Important security boundaries

SatsGrid is not a Lightning escrow system.

A settled Lightning payment cannot be reversed by PayGuard. The reservation and reconciliation system manages application-level accounting; it does not reverse Lightning settlement.

A signed receipt proves that a signing key signed its contents. It does not independently establish inference correctness, model honesty, or Lightning settlement finality.

Provider endpoint validation during discovery also does not yet eliminate every possible DNS rebinding or redirect-chain risk at the final outbound connection boundary.

---

### Demonstrated acceptance environment

The  demonstration uses:

- A local SatsGrid node.
- Ollama as the configured inference backend.
- `llama3.2:latest` as the local model.
- Polar Regtest for Lightning development and payment testing.
- LND for the Regtest Lightning integration.

The demonstration covers the payment authorization flow, inference execution, receipt generation and verification, session functionality, and operator monitoring.

The current demonstration environment has one locally hosted model and does not have external Nostr providers registered. The provider discovery and CLI selection functionality is implemented, but the demo should not be interpreted as evidence of a populated live multi-provider marketplace.

---

## Technical Boundaries and Known Limitations

We believe technical transparency is essential, particularly for a permissionless payment and compute system.

The following are known boundaries of the current implementation.

1. **Dynamic routing:** Dynamic remote provider selection is implemented at the CLI tier through `satsgrid call`. The Playground and standard `/v1/chat/completions` endpoint use the node's configured upstream backend.

2. **Provider health:** Provider health currently uses passive 24-hour freshness TTLs rather than a continuous background active-ping service.

3. **SSRF hardening:** Discovered endpoints are validated, but DNS rebinding and redirect-chain protections at the final outbound connection boundary remain future hardening work.

4. **Receipt verification:** Ed25519 signatures prove that a signing key signed the receipt. They do not independently prove semantic inference correctness or Lightning settlement finality.

5. **Lightning settlement:** PayGuard manages application-level accounting and reconciliation. It is not an escrow mechanism and cannot reverse a settled Lightning payment.

6. **Speech recognition:** Browser speech recognition depends on browser support. Some Chromium-based implementations may use browser-vendor cloud transcription services. Desktop Firefox requires an external speech engine or another supported input method.

7. **External provider availability:** The demonstration environment uses a local Ollama model. A live network with multiple independent providers requires participating operators to register and advertise their services.

These limitations define the current scope and provide a roadmap for further development.

---

## Getting Started

You can run a local SatsGrid node and experiment with the inference and payment authorization workflow.

### Prerequisites

- Rust 1.85 or later.
- Cargo.
- [Ollama](https://ollama.com/) or another OpenAI-compatible inference backend.
- A Lightning backend for payment integration, or the Mock backend for local development.

### 1. Clone the repository

```bash
git clone <SATSGRID_REPOSITORY_URL>
cd satsgrid
```

### 2. Build SatsGrid

```bash
cargo build --release
```

The compiled binary will be available at:

```text
target/release/satsgrid
```

### 3. Prepare an inference backend

For a local Ollama setup:

```bash
ollama pull llama3.2
```

Ensure the Ollama service is running and accessible at:

```text
http://127.0.0.1:11434
```

### 4. Configure the node

Copy the example configuration:

```bash
cp config/node.example.toml config/node.toml
```

The default example configuration uses:

- `127.0.0.1:8080` for the SatsGrid node.
- `http://127.0.0.1:11434` for the Ollama upstream.
- The Mock Lightning backend for local testing.

For Polar Regtest and LND integration, refer to:

```text
config/regtest-node.example.toml
```

And follow the demonstration runbook.

### 5. Start the SatsGrid node

```bash
./target/release/satsgrid node start \
  --config config/node.toml
```

### 6. Make an inference request

In another terminal:

```bash
./target/release/satsgrid call \
  --node http://127.0.0.1:8080 \
  --model llama3.2 \
  --prompt "What is Bitcoin Lightning?"
```

With the Mock backend, the CLI can automatically settle the simulated invoice challenge and continue the inference workflow without spending real satoshis.

---

## Developer Integration

SatsGrid is designed to integrate with existing applications and autonomous agents.

The OpenAI-compatible API provides a familiar interface for developers.

### Example API request

```bash
curl http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "llama3.2",
    "messages": [
      {
        "role": "user",
        "content": "Explain how Lightning payments work."
      }
    ]
  }'
```

For protected endpoints, clients must complete the L402 challenge and provide valid authorization.

### Integration documentation

For detailed examples and payment workflows, see:

- [Developer API Quickstart](docs/quickstart.md)
- [Demo Runbook](docs/demo-runbook.md)

The API guide covers:

- HTTP 402 challenge handling.
- BOLT11 invoice payment flows.
- L402 authorization.
- Session-based inference.
- cURL examples.
- Python `httpx` examples.
- Node.js `fetch` examples.
- SSE streaming.
- Receipt verification.
- Security considerations.

---

## Docker Deployment

SatsGrid provides a Docker Compose development setup.

From the project directory:

```bash
cd docker
docker compose up -d
```

Pull the example model into the containerized Ollama instance:

```bash
docker exec -it satsgrid-ollama ollama pull llama3.2
```

The Docker setup provides a convenient environment for running the node and local inference backend together.

---

## Project Structure

The project is organized around separate components for gateway operations, payment integration, inference, accounting, and application interfaces.

A high-level view:

```text
satsgrid/
├── config/
│   ├── node.example.toml
│   └── regtest-node.example.toml
├── docs/
│   ├── demo-runbook.md
│   └── quickstart.md
├── docker/
├── src/
│   ├── node/
│   │   ├── lightning/
│   │   └── ...
│   ├── ...
│   └── main.rs
├── Cargo.toml
└── README.md
```

Refer to the repository for the complete and current module layout.

---

## Roadmap

SatsGrid establishes the foundations for a more open AI compute ecosystem.

Potential next development areas include:

- Expanding the network of independent compute providers.
- Strengthening provider health monitoring.
- Hardening outbound provider connections against DNS rebinding and redirect attacks.
- Improving provider selection using reliability, latency, and pricing signals.
- Expanding SDK support and developer integrations.
- Improving independent receipt verification and audit tooling.
- Expanding operator monitoring and accounting capabilities.
- Improving browser accessibility and speech interaction.
- Supporting additional inference backends and deployment environments.

These are future development directions, not claims that every capability is already implemented.

---

## Why Bitcoin Lightning?

Bitcoin Lightning is a natural fit for request-level AI payments.

Inference can be consumed in small, variable amounts. Traditional subscription and card-based billing models are not always ideal for individual requests, automated agents, or independent compute providers.

Lightning provides:

- Fast payment settlement.
- Small-value payment support.
- Programmable payment workflows.
- A globally accessible Bitcoin payment network.
- An alternative to conventional card-based billing.

By combining Lightning with L402, SatsGrid makes payment authorization part of the API interaction itself.

This creates opportunities for developers to build software that can pay for compute programmatically.

---

## Why This Matters for Africa

Access to AI infrastructure should not depend on where a developer lives or which international payment methods they can access.

Across Africa, developers, students, researchers, and startups are building products with limited resources and often face barriers when accessing international infrastructure.

SatsGrid explores a different approach.

By combining open-weight models, independent compute providers, and Bitcoin Lightning payments, SatsGrid aims to reduce the friction involved in accessing and monetizing AI compute.

Our Africa-first perspective is not about limiting the network to one continent. It is about building infrastructure that can work globally while addressing the barriers faced by developers in emerging markets.

We believe developers should be able to access compute, providers should be able to monetize available resources, and applications should be able to pay programmatically without depending on a single centralized billing system.

---

---

## Contributing

Contributions, feedback, security reviews, and technical discussions are welcome.

If you are interested in contributing:

1. Explore the repository and existing documentation.
2. Review open issues and identify an area where you can contribute.
3. Discuss significant architectural changes before implementing them.
4. Submit focused pull requests with appropriate tests and documentation.

---

## License and Attribution

SatsGrid is distributed under the terms of the **MIT License**.


Please refer to the repository's license files for the applicable licensing and attribution terms.

---

\<p align="center">
&#x20; \<strong>SatsGrid\</strong>
&#x20; \<br />
&#x20; Permissionless AI Inference. Paid in Sats. Accessible by Design.
&#x20; \<br />\<br />
&#x20; Building a more open, accessible, and permissionless AI compute ecosystem.
\</p>
