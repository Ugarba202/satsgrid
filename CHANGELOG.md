# Changelog

All notable changes to SatsGrid will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Persistent Storage Foundation**: SQLite storage engine configured with WAL mode, busy timeout, and automated transactional migrations.
- **Single-Use Payment Authorization**: Persistent replay protection enforcing primary-key deduplication on payment hashes with atomic state transitions (`reserved` -> `settled` / `consumed`).
- **Durable Session Budget Accounting**: SQLite-backed session accounting with two-phase reservations, financial invariants conservation, and append-only audit ledgers.
- **Durable Inference Request Lifecycle**: Central state machine tracking inference requests, linking payment hashes, session IDs, reservation IDs, and model parameters.
- **Monitored Stream Delivery**: Streaming SSE wrapper with drop guards to prevent premature completion marking and ensure client disconnects are recorded accurately.
- **Conservative Crash Recovery**: Startup recovery routine safely releasing definitely undispatched reservations while locking ambiguous in-flight executions under `reconciliation_required = 1`.
- **Database Idempotency Enforcement**: Partial unique index on `(session_id, idempotency_key)` preventing concurrent duplicate request execution.
- Initial project scaffolding with Axum, Tokio, and modular crate architecture.
- Core domain types, error taxonomy, and typed configurations.
- Modular layout for Node, Client, CLI, L402 Gate, and Lightning abstractions.
