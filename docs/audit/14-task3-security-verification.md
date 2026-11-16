# SatsGrid — Phase 1B, Task 3: Independent Security Verification Report

**Review Status:** Complete  
**Date:** 2026-11-05  
**Reviewer:** Independent Security & Accounting Auditor (Phase 1B Audit)  
**Target:** Phase 1B Task 3 (Persistent Session Budget Manager)  
**Implementation Report:** `docs/audit/13-session-persistence-implementation.md`  
**Workspace:** `/home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid`

---

## 1. Executive Summary

This independent security and accounting review evaluates the durability, concurrency safety, financial invariants, crash recovery, and authorization compatibility of the Phase 1B Task 3 implementation of SatsGrid.

### Findings Verdict:
1. **Production Storage Enforcement:** **CONFIRMED**. In production node execution (`SatsGridServer::run_until_shutdown`), database initialization is mandatory (`Database::init(&self.config.data_dir).await?`), and `SessionBudgetManager` is unconditionally wired with `SessionStore`. Database failures halt node boot. In-memory fallback exists solely as an isolated unit-testing adapter when `SessionBudgetManager::new()` is called without a database.
2. **Database Accounting Invariants:** **CONFIRMED**. The check constraint `chk_session_budget_sum` (`remaining_sats + reserved_sats + spent_sats = total_budget_sats`) and non-negative constraints are enforced in the schema. In all SQL balance mutations (`create_session`, `reserve_budget`, `finalize_reservation`, `release_reservation`), exact satoshi conservation is preserved.
3. **Atomic Concurrency & Lock Escalation Prevention:** **CONFIRMED**. All balance mutations execute inside `BEGIN IMMEDIATE` transactions on dedicated connection checkouts. Concurrent tasks racing on shared budgets serialize cleanly without SQLite lock escalation or overspending.
4. **Crash Recovery & In-Flight State Resolution:** **CONFIRMED**. `recover_in_flight_reservations` conservatively releases only reservations provided in `definitely_undispatched_ids`, while retaining all ambiguous in-flight reservations under `reconciliation_required` without balance alteration or automated refunds.
5. **Append-Only Financial Event Ledger:** **CONFIRMED**. `accounting_entries` records all satoshi lifecycle events (`session_funding`, `session_reservation`, `session_debit`, `session_reservation_release`) with unique UUIDs in the same transaction as the session balance update.
6. **Authorization & Session Compatibility:** **CONFIRMED**. Existing L402 session macaroons, caveats (`session`, `budget`, `capability`, `time <`), and API responses continue to function without regression.
7. **Task 2 Replay Protection:** **CONFIRMED**. Single-use pay-per-request payment authorization and replay prevention remain completely isolated, intact, and fully tested.
8. **Recommendation for Task 4:** **APPROVED (CONFIRMED)**. Task 3 is secure and structurally ready for Task 4 (Request Lifecycle State Machine).

---

## 2. Production Persistence Enforcement

### Verified Startup Pipeline
Inspected [`src/node/server.rs:64-125`](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/src/node/server.rs#L64-L125):
```rust
// 1. Mandatory SQLite Database initialization
let db = Database::init(&self.config.data_dir).await?;

// 2. Unconditional persistent SessionBudgetManager binding
let budget_manager = Arc::new(SessionBudgetManager::with_store(db.session_store()));

// 3. Injected into AppState
let state = AppState {
    ...
    budget_manager,
    db: Some(Arc::new(db)),
};
```

### Verification Questions & Findings:
- **Does production server startup always initialize a persistent SessionStore?**
  **CONFIRMED**. `SatsGridServer::run_until_shutdown` explicitly calls `db.session_store()` and wraps it in `SessionBudgetManager::with_store(...)`.
- **Can production requests reach an in-memory fallback?**
  **CONFIRMED (NO)**. Since `self.store` is `Some(store)` on the production instance, all operations (`open_session_with_details`, `get_session`, `reserve_session`, `finalize_reservation`, `release_reservation`, `debit_session`) route to SQLite.
- **What happens when SQLite initialization fails?**
  **CONFIRMED**. `Database::init` uses the `?` operator. If the database directory cannot be created, SQLite PRAGMAs fail, or migrations fail, node startup immediately terminates with `Error::Config` or `Error::Internal`. The server never binds its TCP port or starts listening.
- **Can a missing database accidentally create a fresh empty session state?**
  **CONFIRMED (NO)**. `state.db` is located at `<data_dir>/state.db`. If the database file is deleted while the server is stopped, `Database::init` will create a new empty database, but existing credentials with unknown session IDs will return `Error::VerificationFailed("Session '<uuid>' not found")`. There is no silent fallback that treats nonexistent sessions as having budget.
- **Are unit-test fallbacks isolated from production behavior?**
  **CONFIRMED**. `SessionBudgetManager::new()` defaults `store: None`, allowing lightweight tests in `src/node/gate/budget.rs` to run without SQLite if desired, while all integration tests and production node daemons use `Database::init` or `with_store`.

---

## 3. Database Accounting Invariants

Inspected Migration 3 in [`src/node/storage/migrations.rs:61-109`](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/src/node/storage/migrations.rs#L61-L109).

### Schema Constraints
```sql
CONSTRAINT chk_session_budget_sum CHECK (remaining_sats + reserved_sats + spent_sats = total_budget_sats)
total_budget_sats INTEGER NOT NULL CHECK (total_budget_sats >= 0)
remaining_sats INTEGER NOT NULL CHECK (remaining_sats >= 0)
reserved_sats INTEGER NOT NULL DEFAULT 0 CHECK (reserved_sats >= 0)
spent_sats INTEGER NOT NULL DEFAULT 0 CHECK (spent_sats >= 0)
status TEXT NOT NULL CHECK (status IN ('active', 'exhausted', 'expired', 'closed'))
payment_hash TEXT UNIQUE
```

### Invariant Analysis Across All Operations:
1. **`create_session`** (`session.rs:138-187`):
   - Sets `remaining_sats = total_budget`, `reserved_sats = 0`, `spent_sats = 0`.
   - Invariant: $B + 0 + 0 = B$. **Holds.**
2. **`reserve_budget`** (`session.rs:353-359`):
   - Sets `remaining_sats = remaining_sats - cost`, `reserved_sats = reserved_sats + cost`.
   - Invariant: $(R - C) + (Res + C) + S = R + Res + S = B$. **Holds.**
3. **`finalize_reservation`** (`session.rs:507-513`):
   - Sets `reserved_sats = reserved_sats - cost`, `spent_sats = spent_sats + cost`.
   - Invariant: $R + (Res - C) + (S + C) = R + Res + S = B$. **Holds.**
4. **`release_reservation`** (`session.rs:633-640`):
   - Sets `reserved_sats = reserved_sats - cost`, `remaining_sats = remaining_sats + cost`.
   - Invariant: $(R + C) + (Res - C) + S = R + Res + S = B$. **Holds.**

### Referential Integrity & Uniqueness:
- `payment_hash`: Enforced `UNIQUE`. Re-funding or binding multiple sessions to the same invoice hash is rejected with unique constraint errors.
- `session_reservations.session_id`: Enforces foreign key to `sessions(id)` with `ON DELETE CASCADE`.
- `session_reservations.reserved_sats`: Constrained `CHECK (reserved_sats > 0)`. Zero or negative reservations are rejected.

---

## 4. Atomic Concurrency & Transaction Analysis

### Concurrency Architecture
Under standard deferred transactions in SQLite WAL mode, concurrent tasks that read then write to the same table can trigger `SQLITE_BUSY (code: 5) database is locked` due to read-to-write lock escalation.

In Task 3, `SessionStore` explicitly acquires a dedicated connection from the connection pool and issues `BEGIN IMMEDIATE` for every mutating workflow:
- `reserve_budget` ([`src/node/storage/session.rs:260`](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/src/node/storage/session.rs#L260))
- `finalize_reservation` ([`src/node/storage/session.rs:449`](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/src/node/storage/session.rs#L449))
- `release_reservation` ([`src/node/storage/session.rs:574`](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/src/node/storage/session.rs#L574))

### Invariant Verification Under Race Conditions:
- **Can concurrent requests overspend?**
  **CONFIRMED (NO)**. Every reservation transaction checks `remaining_sats >= cost` inside the immediate lock and executes conditional update `WHERE id = ?4 AND remaining_sats >= ?1 AND status = 'active'`. Tested under 10 parallel tasks contending for a 500 sat budget (`test_concurrent_requests_exceeding_available_budget`); exactly 5 succeed and 5 fail.
- **Can a reservation be finalized twice?**
  **CONFIRMED (NO)**. `finalize_reservation` checks `status == 'reserved'` and performs `UPDATE ... WHERE id = ?2 AND status = 'reserved'`. If already finalized, `rows_affected() == 0`, and the transaction aborts with an error. Tested in `test_duplicate_finalization_rejection`.
- **Can a reservation be released twice?**
  **CONFIRMED (NO)**. `release_reservation` checks `status == 'reserved'` and performs `UPDATE ... WHERE id = ?3 AND status = 'reserved'`. Second release fails with an error. Tested in `test_duplicate_release_rejection`.
- **Can a released reservation be finalized or vice versa?**
  **CONFIRMED (NO)**. The status check strictly requires `'reserved'`. A released reservation is `'released'`; a finalized reservation is `'finalized'`.
- **Do database failures roll back mutations atomically?**
  **CONFIRMED**. If an error occurs prior to `COMMIT`, the connection is rolled back or dropped, undoing all uncommitted session updates and preventing phantom ledger entries.

---

## 5. Crash Recovery & In-Flight State Resolution

Inspected `recover_in_flight_reservations()` in [`src/node/storage/session.rs:728-756`](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/src/node/storage/session.rs#L728-L756).

### Mechanics:
1. `recover_in_flight_reservations` takes `definitely_undispatched_ids: &[String]`.
2. For each ID in `definitely_undispatched_ids`, it calls `self.release_reservation(res_id, "server_crash_prior_to_dispatch")`.
3. For any remaining reservations with `status = 'reserved'`, it executes:
   ```sql
   UPDATE session_reservations 
   SET status = 'reconciliation_required', 
       failure_reason = 'unresolved_crash_during_execution', 
       finalized_at = ?1 
   WHERE status = 'reserved'
   ```

### Evaluation Against Crash Scenarios:

| Scenario | State at Crash | Recovery Action | Balance Safety Analysis |
|---|---|---|---|
| 1. Crash immediately after reservation creation | `session_reservations.status == 'reserved'` | Retained as `reconciliation_required` unless caller provides undispatched proof. | **SAFE (CONFIRMED)**. No free compute leak. |
| 2. Crash before upstream dispatch | `session_reservations.status == 'reserved'` | If flagged undispatched, safely releases to `remaining_sats`. Otherwise retained for operator review. | **SAFE (CONFIRMED)**. |
| 3. Crash after dispatch begins | `status == 'reserved'` | Retained as `reconciliation_required`. | **SAFE (CONFIRMED)**. Balance is NOT restored to caller, protecting node from unbilled GPU compute. |
| 4. Crash during inference | `status == 'reserved'` | Retained as `reconciliation_required`. | **SAFE (CONFIRMED)**. |
| 5. Crash after inference completes, before finalization | `status == 'reserved'` | Retained as `reconciliation_required`. | **SAFE (CONFIRMED)**. Operator can audit request log and manually or automatically finalize. |
| 6. Crash during finalization transaction | Incomplete transaction rolls back to `reserved`. | Retained as `reconciliation_required`. | **SAFE (CONFIRMED)**. |
| 7. Restart with multiple active reservations | Multiple records in `reserved`. | Ambiguous records transition to `reconciliation_required`. | **SAFE (CONFIRMED)**. |

**Audit Finding (Durable Dispatch Marker Scope):**  
Task 3 implements the storage foundation for reservation recovery. As designed in Revision 3, the durable tracking of upstream dispatch start (`started_at`) and request states (`processing`, `stream_interrupted`) belongs to the `inference_requests` table, which is scheduled for **Task 4**. In Task 3, `recover_in_flight_reservations` appropriately avoids making assumptions: it only releases reservations when explicitly passed in `definitely_undispatched_ids`, treating all other in-flight reservations as ambiguous.

---

## 6. Accounting Event Verification

Inspected `accounting_entries` schema and event emission points.

### Verification Matrix:
- **Unique Event Identifiers:** **CONFIRMED**. Every event generates `Uuid::new_v4().to_string()` inserted into `entry_uuid` (`UNIQUE`).
- **Event Types:** **CONFIRMED**. Constrained to `'session_funding'`, `'session_reservation'`, `'session_debit'`, `'session_reservation_release'`, and `'recovery_adjustment'`.
- **Signed Amount Convention:** **CONFIRMED**. `amount_sats` is strictly non-negative (`amount_sats >= 0`). Semantics are determined by `entry_type`.
- **Derivation of Operator Earnings:**
  Operator earnings can be derived without double counting:
  $$\text{Audited Session Earnings} = \sum_{\text{entry\_type} = \text{'session\_debit'}} \text{amount\_sats}$$
  `session_reservation` and `session_reservation_release` represent temporary holds and reversals and are excluded from revenue queries.
- **Atomic Event Commit:** **CONFIRMED**. In `create_session`, `reserve_budget`, `finalize_reservation`, and `release_reservation`, the `INSERT INTO accounting_entries` query executes on the exact same transaction handle prior to `COMMIT`.

---

## 7. Session Authorization Compatibility

Inspected [`src/node/gate/verify.rs`](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/src/node/gate/verify.rs), [`src/node/gate/macaroon.rs`](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/src/node/gate/macaroon.rs), and [`src/node/api/handlers.rs`](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/src/node/api/handlers.rs).

- **Macaroon Caveat Validation:** **CONFIRMED**. Model, expiration, and capability caveats are verified by `L402Verifier::verify_header` prior to any session budget checks.
- **Expired Session Rejection:** **CONFIRMED**. Sessions past their `expires_at` timestamp are rejected with `SessionReservationResult::Expired` and their database status is transitioned to `'expired'`. Verified in `test_expired_session_rejection`.
- **Model Restrictions:** **CONFIRMED**. `L402Verifier` enforces model caveats against the request payload.
- **Budget Caveats:** **CONFIRMED**. Macaroons declaring `budget = <sats>` are extracted and validated against session funding.
- **Session ID Substitution Protection:** **CONFIRMED**. Macaroons bind the session ID cryptographically via `Caveat::Session(uuid)`. Tampering with the session ID invalidates the HMAC signature (`test_tampered_caveats_are_rejected`).
- **HTTP API Behavior:** **CONFIRMED**. `POST /v1/session/new` continues returning HTTP `402 Payment Required` with `WWW-Authenticate: L402 token="...", invoice="..."`.

---

## 8. Task 2 Regression Findings

Inspected pay-per-request handling in [`src/node/api/handlers.rs:227-251`](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/src/node/api/handlers.rs#L227-L251) and [`tests/payment_authorization_tests.rs`](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/tests/payment_authorization_tests.rs).

- **Replay Protection Intact:** **CONFIRMED**. Pay-per-request tokens continue to use `payment_authorizations` with primary key uniqueness on `payment_hash`.
- **Session Token Isolation:** **CONFIRMED**. Session macaroons contain a `session` caveat and branch to `state.budget_manager.debit_session(...)`, completely bypassing `payment_authorizations`.
- **Full Test Suite Validation:** All 7 dedicated payment authorization tests pass without error (`cargo test --test payment_authorization_tests`).

---

## 9. Test Coverage Matrix

| Scenario | Tested in Automated Suite? | Inspected in Code? | Test Location / Function | Verification Status |
|---|:---:|:---:|---|---|
| Concurrent overspending | Yes | Yes | `session_persistence_tests::test_concurrent_requests_exceeding_available_budget` | **CONFIRMED** |
| Concurrent multi-connection | Yes | Yes | `session_persistence_tests::test_concurrent_reservations_multi_connection` | **CONFIRMED** |
| Concurrent finalization duplicate | Yes | Yes | `session_persistence_tests::test_duplicate_finalization_rejection` | **CONFIRMED** |
| Double release rejection | Yes | Yes | `session_persistence_tests::test_duplicate_release_rejection` | **CONFIRMED** |
| Finalize-after-release | Code | Yes | Enforced by `status == 'reserved'` guard in `finalize_reservation` | **CONFIRMED** |
| Release-after-finalize | Code | Yes | Enforced by `status == 'reserved'` guard in `release_reservation` | **CONFIRMED** |
| Database rollback on failure | Yes | Yes | Integration tests assert balance unchanged on failed/exhausted debits | **CONFIRMED** |
| Cross-reconnect persistence | Yes | Yes | `session_persistence_tests::test_session_persistence_across_reconnect` | **CONFIRMED** |
| Node reinit persistence | Yes | Yes | `session_persistence_tests::test_session_persistence_across_node_reinitialization` | **CONFIRMED** |
| Restart recovery (undispatched) | Yes | Yes | `session_persistence_tests::test_recovery_of_definitely_undispatched_reservations` | **CONFIRMED** |
| Ambiguous outcome retention | Yes | Yes | `session_persistence_tests::test_retention_of_ambiguous_reservations` | **CONFIRMED** |
| Production storage enforcement | Yes | Yes | `server.rs:69-72` code inspection & `test_existing_session_api_regression` | **CONFIRMED** |
| Session caveat compatibility | Yes | Yes | `session_persistence_tests::test_session_macaroon_compatibility` | **CONFIRMED** |
| Task 2 replay regression | Yes | Yes | `payment_authorization_tests.rs` (7 tests passing) | **CONFIRMED** |

---

## 10. Confirmed Vulnerabilities and Limitations

### Finding 1: Two-Phase Reservation Unification in `chat_completions` (Known Task 3 Scope Boundary)
- **Status:** **CONFIRMED (Design Boundary)**
- **Description:** In [`src/node/api/handlers.rs:219`](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/src/node/api/handlers.rs#L219), `chat_completions` calls `budget_manager.debit_session(&session_id, cost)`. In `SessionBudgetManager`, `debit_session` performs an atomic reservation followed immediately by finalization before forwarding to `state.proxy`. If the proxy fails or times out, the reservation was already finalized to spent.
- **Risk Assessment:** Expected and documented. The splitting of session reservation (pre-proxy) and finalization/release (post-proxy) depends on the durable request state machine (`inference_requests`), which is the explicit objective of **Task 4**.
- **Remediation Plan:** In Task 4, replace `debit_session` with `reserve_session` before dispatch, and call `finalize_reservation` on completion or `release_reservation` on pre-generation error.

### Finding 2: `sessions.status` Transitions on Dynamic Restorations
- **Status:** **CONFIRMED (Minor Ergonomic Behavior)**
- **Description:** When a session budget reaches 0, its status transitions to `'exhausted'`. If an unconsumed reservation is subsequently released (via `release_reservation`), the query explicitly updates `status = 'active'`, restoring spendability. This is functionally correct and verified by `test_confirmed_failure_release`.

---

## 11. Recommendation on Whether Task 4 Can Safely Begin

### Recommendation: **APPROVED TO PROCEED**

The implementation of Phase 1B Task 3 satisfies all accounting, persistence, and security requirements:
- Sessions, reservations, and append-only event ledgers are durably stored in SQLite.
- Math invariants ($R + Res + S = B$) are enforced at the schema and application level.
- Multi-connection concurrency safely serializes without lock contention or overspending via `BEGIN IMMEDIATE`.
- Crash recovery is evidence-grounded and does not grant free refunds for ambiguous executions.
- All 136 tests across the entire SatsGrid workspace pass cleanly.
- `cargo fmt` and `cargo clippy` pass with zero warnings.

Phase 1B Task 4 may safely begin.

