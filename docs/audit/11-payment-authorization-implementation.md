# SatsGrid — Phase 1B, Task 2: Payment Authorization & Replay Prevention Implementation Report

**Status:** Complete  
**Date:** 2026-11-05  
**Scope:** Phase 1B, Task 2 only (Atomic Payment Authorization & Replay Prevention)  
**Workspace:** `/home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid`

---

## 1. Executive Summary

Task 2 implements a durable SQLite-backed payment authorization store in SatsGrid, enforcing atomic single-use reservation semantics for pay-per-request L402 payment hashes.

Settled payment hashes can no longer be replayed to obtain free inference:
1. Every pay-per-request credential presentation must atomically transition the payment hash state in SQLite from untracked to `reserved`.
2. Concurrent duplicate requests race at the database level (`INSERT INTO payment_authorizations` with primary key uniqueness); exactly one succeeds, while all racing duplicates fail with duplicate conflict errors.
3. Replayed requests receive an HTTP `401 Unauthorized` with a fresh L402 challenge.
4. Preimages are **never** stored in the database.
5. Session tokens carry `auth_type: "session"` and bypass the single-use payment authorization store, continuing to debit their dedicated session budgets as designed.
6. Authorizations survive process restarts and SQLite connection closures.

---

## 2. Schema: Migration 0002

File: [`src/node/storage/migrations.rs`](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/src/node/storage/migrations.rs)

```sql
CREATE TABLE IF NOT EXISTS payment_authorizations (
    payment_hash TEXT PRIMARY KEY,
    auth_type TEXT NOT NULL,
    state TEXT NOT NULL,
    idempotency_key TEXT,
    request_id TEXT,
    reserved_at INTEGER NOT NULL,
    settled_at INTEGER,
    consumed_at INTEGER,
    expires_at INTEGER,
    metadata_json TEXT
);

CREATE INDEX IF NOT EXISTS idx_payment_authorizations_state
    ON payment_authorizations (state);

CREATE INDEX IF NOT EXISTS idx_payment_authorizations_idempotency
    ON payment_authorizations (idempotency_key)
    WHERE idempotency_key IS NOT NULL;
```

### Security and Privacy Guarantees:
- **No Preimages**: The table only records `payment_hash`. Preimages are verified in-memory during HMAC/invoice settlement and discarded immediately.
- **Strict Typing**: State transitions use `PaymentAuthState` (`reserved`, `settled`, `consumed`).
- **Deterministic Migrations**: Migration 2 is executed in numerical order after migration 1 during engine startup inside an explicit transaction.

---

## 3. Storage Layer: `PaymentStore`

File: [`src/node/storage/payment.rs`](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/src/node/storage/payment.rs)

The `PaymentStore` provides atomic, durable primitives:

```rust
pub async fn reserve_payment_hash(
    &self,
    payment_hash: &str,
    auth_type: &str,
    idempotency_key: Option<&str>,
    request_id: Option<&str>,
    expires_at: Option<i64>,
) -> Result<ReservationResult>;

pub async fn mark_settled_and_consumed(
    &self,
    payment_hash: &str,
) -> Result<()>;

pub async fn get_authorization_state(
    &self,
    payment_hash: &str,
) -> Result<Option<PaymentAuthState>>;
```

### Atomic Concurrency Control:
- `reserve_payment_hash` attempts an atomic `INSERT INTO payment_authorizations`.
- If an existing entry exists (SQLite error 1555 / `SQLITE_CONSTRAINT_PRIMARYKEY` / `UNIQUE constraint failed`), it performs a read of the existing state and returns `ReservationResult::AlreadyUsed { state, reserved_at }`.
- Under 20 parallel threads racing for the identical payment hash, exactly one acquires `ReservationResult::Reserved` while the other 19 receive `AlreadyUsed`.

---

## 4. L402 Request Handling Integration

Files:
- [`src/node/api/handlers.rs`](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/src/node/api/handlers.rs)
- [`src/node/gate/macaroon.rs`](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/src/node/gate/macaroon.rs)

### Handshake and Verification Sequence:
1. Parse `Authorization: L402 <macaroon>:<preimage>` or `LSAT <macaroon>:<preimage>`.
2. Verify macaroon HMAC integrity and validate model/capability caveats against request.
3. Verify preimage matches payment hash ($H(\text{preimage}) == \text{payment\_hash}$) and check settlement with the Lightning backend.
4. **Session Detection**: Check if macaroon contains session caveat (`session_id`).
   - If **Session**: Validate session token and debit session budget. Do **not** record in single-use `payment_authorizations`.
   - If **Pay-per-request**:
     - Query `AppState.db.payment_store()`.
     - Execute `payment_store.reserve_payment_hash(payment_hash, "pay_per_request", ...)`.
     - If `AlreadyUsed`: Log replay detection warning and return `401 Unauthorized` with a new `WWW-Authenticate: L402` challenge.
     - If `Reserved`: Mark settled/consumed and proceed to upstream inference proxy.

---

## 5. Verification & Test Suite

### New Integration Tests: [`tests/payment_authorization_tests.rs`](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/tests/payment_authorization_tests.rs)

1. `test_first_authorization_reservation_succeeds`: Validates initial reservation returns `ReservationResult::Reserved`.
2. `test_sequential_duplicate_reservation_is_rejected`: Validates second sequential reservation with same payment hash returns `AlreadyUsed`.
3. `test_concurrent_duplicate_reservations_allow_exactly_one_winner`: Spawns 20 concurrent Tokio tasks with barriers racing to reserve the same hash; asserts exactly 1 winner and 19 `AlreadyUsed`.
4. `test_same_payment_hash_with_different_idempotency_key_is_rejected`: Verifies different idempotency keys cannot bypass single-use hash restriction.
5. `test_consumed_authorization_survives_reconnect_and_reinit`: Confirms authorizations survive database disconnect and re-initialization.
6. `test_session_credentials_do_not_enter_payment_authorizations`: Verifies session tokens bypass single-use storage table entirely.
7. `test_e2e_replay_rejected_in_chat_completions`: Full Axum route test through `/v1/chat/completions`:
   - First request with valid paid L402 succeeds (`200 OK`).
   - Exact replay of the same token returns `401 Unauthorized` with fresh invoice challenge.

### Test Results

- `cargo test --test payment_authorization_tests`: **7 passed, 0 failed**
- `cargo test --test storage_tests`: **11 passed, 0 failed**
- `cargo test --workspace`: **119 passed, 0 failed across 19 integration test targets**
- `cargo fmt --all -- --check`: **Clean (exit code 0)**
- `cargo clippy --all-targets --all-features -- -D warnings`: **Clean (exit code 0)**

---

## 6. Constraints Adherence

- **Original repository untouched**: `~/Desktop/Ucee/BossBattle/nodus` remains completely untouched.
- **No Git commits**: No `git commit` or `git push` executed.
- **PayGuard deferred**: No PayGuard or out-of-scope escrow components created.
- **Scope limited to Task 2**: Persistent session budgets, hold invoices, and upstream failure refund lifecycles are deferred to their designated future tasks.

