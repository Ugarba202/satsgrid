# Security Policy

## Reporting a Vulnerability

Security is paramount to SatsGrid. If you discover a security issue or vulnerability concerning L402 payment verification, macaroon validation, database invariants, or sensitive data leakage:

1. Please do not open a public GitHub issue.
2. Email security concerns to `hamza.00dev1@gmail.com` or submit a private GitHub security advisory.
3. We will respond promptly with an acknowledgment and mitigation timeline.

## Security Architecture & Invariants

- **Payment Authorization**: Pay-per-request payment hashes are single-use. Replays are detected and rejected at the database constraint level.
- **Session Conservation**: Session budgets strictly conserve satoshis: `remaining_sats + reserved_sats + spent_sats = total_budget_sats`. Balances are guaranteed non-negative.
- **Privacy Posture**: User prompts, system instructions, model completions, preimages, and macaroon root keys remain strictly ephemeral in RAM and are never persisted to disk or emitted in logs.
- **Crash Recovery**: Financial reservations are never released automatically without proof that upstream compute was never initiated (`started_at IS NULL`).
