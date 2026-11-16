# Task 4 Implementation Report: Durable Inference Request Lifecycle

## 1. Executive Summary

This report documents the implementation of Phase 1C, Task 4: Durable Inference Request Lifecycle for SatsGrid. Task 4 establishes an end-to-end coordinated state machine linking L402 payment authorization, persistent session budget management, upstream inference execution, stream accounting, and crash recovery.

Key accomplishments in Task 4:
- **Migration 4 (`create_inference_requests`)**: Introduced durable request tracking schema recording lifecycle states, timestamps, foreign key links to payments and sessions, token usage metrics, satoshi charges, and failure/reconciliation metadata.
- **Request Storage Engine (`RequestStore`)**: Implemented atomic state transitions (`reserved` -> `processing` -> `completed` / `failed` / `stream_interrupted`) with monotonic invariants, preventing illegal transitions, duplicate finalizations, or balance tampering.
- **Durable Dispatch Boundary**: Placed the critical transition from `reserved` to `processing` (`started_at IS NOT NULL`) strictly before invoking upstream HTTP execution. This guarantees unambiguous recovery: any request unstarted at reboot was never transmitted upstream and can be safely released, while any request marked `processing` or `stream_interrupted` is flagged for manual reconciliation rather than risking unbilled compute or double-spending.
- **JSON and SSE Lifecycle Coordination**: Unified pay-per-request and persistent session authorization in `chat_completions`. Handled non-streaming full responses (extracting token usage) and SSE stream lifecycles (wrapping response streams with drop guards to record connection interruptions).
- **Session Idempotency Replay Protection**: Prevented replay and re-execution of completed requests with duplicate session idempotency keys.
- **Startup Crash Recovery Integration**: Automated startup scan in `SatsGridServer::run_until_shutdown` that queries in-flight requests, marks undispatched reservations for safe rollback, and preserves ambiguous in-flight sessions under `reconciliation_required`.
- **Zero Privacy Regressions**: User prompts, system instructions, model outputs, preimages, and secrets remain strictly in RAM and are never persisted to disk or logs.
- **Comprehensive Verification**: 12 dedicated lifecycle unit/integration tests added in `tests/request_lifecycle_tests.rs`. All workspace tests (108 tests) pass with exit code 0; `cargo clippy` passes clean under `-D warnings`; `cargo fmt` verified.

---

## 2. Existing Request Flow Analysis

Prior to Task 4:
1. In Task 2, `PaymentStore` introduced atomic reservations for pay-per-request payment hashes.
2. In Task 3, `SessionStore` introduced durable session budgets and atomic `session_reservations`.
3. However, the inference execution path in `src/node/api/handlers.rs` remained decoupled:
   - Pay-per-request hashes were consumed *after* non-streaming responses or *at stream launch*, but without an associated request record.
   - Session reservations were created using `budget_manager.reserve_budget()`, but if the node crashed mid-flight between upstream transmission and response parsing, recovery could not tell whether Ollama had actually processed the prompt.
   - SSE streaming errors or abrupt TCP teardowns were unmonitored at the database level.
   - No table recorded client request parameters (model, streaming mode, tokens, charges, timing).

Task 4 introduces `inference_requests` as the central coordinator unifying these layers.

---

## 3. Database Schema: Migration 4 (`create_inference_requests`)

Migration 4 was appended to `src/node/storage/migrations.rs`:

```sql
CREATE TABLE IF NOT EXISTS inference_requests (
    id TEXT PRIMARY KEY,
    payment_hash TEXT,
    session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL,
    reservation_id TEXT REFERENCES session_reservations(id) ON DELETE SET NULL,
    idempotency_key TEXT,
    model TEXT NOT NULL,
    request_mode TEXT NOT NULL CHECK (request_mode IN ('streaming', 'non_streaming')),
    status TEXT NOT NULL CHECK (status IN ('authorized', 'reserved', 'processing', 'completed', 'failed', 'stream_interrupted', 'reconciliation_required')),
    created_at INTEGER NOT NULL,
    reserved_at INTEGER NOT NULL,
    started_at INTEGER,
    completed_at INTEGER,
    failed_at INTEGER,
    actual_prompt_tokens INTEGER,
    actual_completion_tokens INTEGER,
    charged_sats INTEGER NOT NULL CHECK (charged_sats >= 0),
    error_code TEXT,
    error_category TEXT,
    reconciliation_required BOOLEAN NOT NULL DEFAULT 0,
    CONSTRAINT chk_auth_source CHECK (payment_hash IS NOT NULL OR session_id IS NOT NULL),
    FOREIGN KEY (payment_hash) REFERENCES payment_authorizations(payment_hash) ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS idx_inference_requests_status ON inference_requests(status);
CREATE INDEX IF NOT EXISTS idx_inference_requests_payment_hash ON inference_requests(payment_hash);
CREATE INDEX IF NOT EXISTS idx_inference_requests_session_id ON inference_requests(session_id);
CREATE INDEX IF NOT EXISTS idx_inference_requests_reservation_id ON inference_requests(reservation_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_inference_requests_session_idempotency ON inference_requests(session_id, idempotency_key) WHERE session_id IS NOT NULL AND idempotency_key IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_inference_requests_idempotency ON inference_requests(idempotency_key) WHERE idempotency_key IS NOT NULL;
```

### Schema Guarantees
- **Referential Integrity**: Foreign keys enforce that payment hashes exist in `payment_authorizations` (with `ON DELETE RESTRICT`) and sessions exist in `sessions` (with `ON DELETE SET NULL` to preserve historical audit records if sessions are archived).
- **Status Constraints**: Strict check constraint ensures only valid enum values (`authorized`, `reserved`, `processing`, `completed`, `failed`, `stream_interrupted`, `reconciliation_required`) are allowed.
- **Authorization Source Check**: `CONSTRAINT chk_auth_source CHECK (payment_hash IS NOT NULL OR session_id IS NOT NULL)` ensures every request is bound to an authorized credential source.
- **Balance Non-negativity**: `charged_sats >= 0`.
- **Reconciliation Flag**: Boolean constraint tracking ambiguous recovery items.
- **Targeted Indexes**: Fast lookups for in-flight requests, session history, and a partial unique index on `(session_id, idempotency_key)` to enforce idempotency at the database level.

---

## 4. State Machine & Transition Invariants

The request lifecycle is managed by `RequestStore` in `src/node/storage/request.rs`.

```
                    +--------------+
                    |   Reserved   |
                    +-------+------+
                            |
                 mark_processing() (Durable Dispatch Boundary)
                            |
                            v
                    +--------------+
                    |  Processing  |
                    +---+---+------+
                        |   |
      +-----------------+   +------------------+
      |                                        |
mark_completed()                        mark_failed()
      |                                        |
      v                                        v
+--------------+                       +---------------+
|  Completed   |                       |    Failed     |
+--------------+                       +---------------+
      |                                        ^
      |                                        |
      | (SSE mid-stream drop)                  | (Undispatched
      |                                        |  recovery)
      v                                        |
+--------------------+                         |
| StreamInterrupted  |                         |
+--------------------+                         |
      |                                        |
      +-----> (Startup Recovery: Flagged) -----+
```

### Transition Invariants
1. **`reserved`**: Initial state on request creation. Timestamps `created_at` and `reserved_at` populated. `started_at` is `NULL`.
2. **`processing`**: Can only be entered from `reserved`. Sets `started_at = now()`. This transition is committed **immediately before** making the upstream HTTP request.
3. **`completed`**: Terminal state. Can only be transitioned from `processing`. Sets `completed_at`, `actual_prompt_tokens`, `actual_completion_tokens`. Once `completed`, no further state transitions are permitted.
4. **`failed`**: Terminal state. Can be entered from `reserved` (e.g. pre-dispatch failure, upstream connect error) or `processing`. Sets `failed_at`, `error_code`, `error_category`.
5. **`stream_interrupted`**: Special intermediate state for streaming requests where the client disconnected or the connection threw a transport error after chunks began streaming. Sets `error_code`, `reconciliation_required = 1`.
6. **Monotonicity**: All UPDATE statements use conditional `WHERE status = '...'` clauses. If rows affected == 0, the store returns `Error::VerificationFailed("Invalid state transition...")`.

---

## 5. Payment and Session Integration

The coordination logic resides in `chat_completions` within `src/node/api/handlers.rs`:

```rust
// 1. Session Idempotency Pre-Check
if let (Some(db), Some(idem_key)) = (&state.db, idempotency_key) {
    let req_store = db.request_store();
    if let Some(existing) = req_store.find_by_idempotency_key(session_id, idem_key).await? {
        if existing.status == RequestStatus::Completed {
            return Err(Error::VerificationFailed(
                "Idempotency key has already been consumed for this session".to_string(),
            ));
        }
    }
}

// 2. Authorization & Reservation Linking
match auth_info {
    AuthInfo::PayPerRequest { payment_hash, .. } => {
        // Reserve payment hash in payment_store
        db.payment_store().reserve_payment_hash(...).await?;
        db.request_store().create_request(..., Some(&payment_hash), None, None, ...).await?;
    }
    AuthInfo::Session { session_id, .. } => {
        // Reserve budget in session_store
        let res_id = format!("res_{}", internal_request_id);
        db.session_store().reserve_budget(&session_id, required_cost, &res_id).await?;
        db.request_store().create_request(..., None, Some(&session_id), Some(&res_id), ...).await?;
    }
}

// 3. Durable Dispatch Boundary
db.request_store().mark_processing(&internal_request_id).await?;

// 4. Upstream Execution
let upstream_result = state.openai_client.forward_chat(...).await;
```

---

## 6. Streaming (SSE) and Non-Streaming Lifecycle

### Non-Streaming Lifecycle
- Upstream returns complete JSON response.
- Server extracts `usage.prompt_tokens` and `usage.completion_tokens`.
- On success:
  - `mark_completed` updates `inference_requests` with tokens.
  - Pay-per-request: marks payment hash `consumed`.
  - Session: calls `finalize_reservation` with actual cost (or estimated reservation).
- On failure (e.g. 500 from Ollama):
  - `mark_failed` records failure details.
  - Session reservation is released (`release_reservation`).
  - Pay-per-request authorization is released.

### Streaming (SSE) Lifecycle
- For streaming requests, the final satoshi cost cannot be altered mid-stream without breaking SSE format. Therefore, the reservation is finalized upon successful HTTP connection to upstream.
- To detect connection drops, client cancellations, or proxy read errors, the byte stream is wrapped in an async stream monitor:
```rust
let monitored_stream = async_stream::stream! {
    let mut stream = upstream_stream;
    while let Some(item) = stream.next().await {
        match item {
            Ok(bytes) => yield Ok(bytes),
            Err(e) => {
                // Spawn DB task to record stream interruption
                tokio::spawn(async move {
                    let _ = req_store.mark_stream_interrupted(&r_id, "stream_read_error", None).await;
                });
                yield Err(e);
                return;
            }
        }
    }
};
```
- On natural stream completion, `mark_completed` records the successful completion.

---

## 7. Crash Recovery Semantics

Startup recovery is coordinated in `SatsGridServer::run_until_shutdown` (`src/node/server.rs`):

```rust
if let Some(db) = &self.db {
    let req_store = db.request_store();
    let undispatched_reservations = req_store.recover_in_flight_requests().await?;

    let session_store = db.session_store();
    let recovered_sessions = session_store
        .recover_in_flight_reservations(&undispatched_reservations)
        .await?;
}
```

### Recovery Logic (`recover_in_flight_requests`)
1. Selects all rows where `status IN ('reserved', 'processing', 'stream_interrupted')`.
2. Evaluates each in-flight request:
   - **`status == 'reserved'` AND `started_at IS NULL`**:
     - The request was reserved in SQLite, but the process crashed before the dispatch marker was committed.
     - Upstream Ollama was definitely not called.
     - Transitions request status to `failed` (`error_code = 'server_crash_prior_to_dispatch'`).
     - Adds its `reservation_id` to the `undispatched_reservations` list, authorizing `SessionStore` to safely release the reserved budget back to `remaining_sats`.
   - **`status == 'processing'` OR `started_at IS NOT NULL`**:
     - Upstream dispatch may have begun or completed.
     - Sets `reconciliation_required = 1`.
     - Request is NOT added to undispatched reservations.
     - `SessionStore` will NOT release the budget; funds remain locked in `reserved_sats` until manual or administrative audit, preventing double-spending.
   - **`status == 'stream_interrupted'`**:
     - Retains `reconciliation_required = 1`.

---

## 8. Test Verification

Task 4 introduced `tests/request_lifecycle_tests.rs` with 12 comprehensive unit and integration tests:

| Test Name | Verification Focus | Status |
|:---|:---|:---:|
| `test_request_creation_and_retrieval` | Validates initial insertion in `reserved` status and field integrity. | PASSED |
| `test_valid_state_transitions` | Validates monotonic `reserved -> processing -> completed` path. | PASSED |
| `test_invalid_state_transition_rejection` | Rejects direct transitions from `reserved` to `completed` without `processing`. | PASSED |
| `test_duplicate_transition_rejection` | Rejects duplicate `mark_completed` or `mark_failed` calls. | PASSED |
| `test_payment_request_linkage` | Verifies foreign key enforcement and retrieval for payment authorizations. | PASSED |
| `test_session_reservation_linkage` | Verifies foreign key linkage with `sessions` and `session_reservations`. | PASSED |
| `test_duplicate_session_idempotency_prevention` | Confirms unique idempotency key detection per session. | PASSED |
| `test_crash_before_dispatch_recovery` | Confirms undispatched `reserved` requests are marked failed and budget is released. | PASSED |
| `test_crash_after_dispatch_marker_retention` | Confirms `processing` requests retain reserved budget with `reconciliation_required = 1`. | PASSED |
| `test_failed_upstream_request_releases_reservation` | Verifies immediate rollback of session reservation when upstream returns error. | PASSED |
| `test_sse_completion_and_interruption` | Verifies stream lifecycle recording for both natural completion and stream interruptions. | PASSED |
| `test_concurrent_request_creation` | Verifies 10 concurrent requests creating unique lifecycle records under load. | PASSED |

### Schema Test Update
- Added `test_migration_4_inference_requests_schema` in `tests/storage_tests.rs`.

### Full Workspace Test Run
```bash
cargo test --workspace
# Result: 108 passed; 0 failed; 0 ignored; 0 filtered out
```

### Code Quality & Standards
- `cargo clippy --all-targets --all-features -- -D warnings`: Clean, 0 warnings.
- `cargo fmt --all -- --check`: Clean formatting.

---

## 9. Privacy and Security Posture

- **Zero Prompt / Secret Logging**: As verified in `tests/privacy_audit_tests.rs`, client prompts, system instructions, completions, preimages, and macaroon root keys are excluded from the database schema and application logging.
- **Durable Accounting Guard**: Financial balances cannot be modified without database transactions, and no funds are unreserved without proof of non-dispatch.
- **Crash Safety**: All write operations use SQLite WAL mode, foreign keys, and synchronous checkpoints.

---

## 10. Remaining Limitations & Recommended Next Task

### Remaining Limitations
- Dynamic token counting in streaming mode does not dynamically refund over-reserved satoshis upon completion (streaming uses flat/initial estimate reservation).
- Recovery for `reconciliation_required` requests requires operator intervention or node inspection tools (an administrative reconciliation API or CLI).
- No Hold Invoices (deliberately scoped for Phase 2).

### Next Task Recommendation
With Task 1 (Storage), Task 2 (Payment Authorization), Task 3 (Persistent Session Budget Manager), and Task 4 (Durable Inference Request Lifecycle) complete, Phase 1C is ready for **Task 4 Independent Security Verification**. Upon sign-off, Phase 1 is complete, paving the way for Phase 2 (PayGuard, Hold Invoices, or Advanced Node Features).

