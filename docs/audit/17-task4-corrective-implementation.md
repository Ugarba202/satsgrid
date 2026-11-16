# SatsGrid — Phase 1C, Task 4A: Corrective Implementation Report

**Status:** Complete  
**Date:** 2026-11-05  
**Review Target:** Findings from `docs/audit/16-task4-security-verification.md`  
**Workspace:** `/home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid`

---

## 1. Executive Summary

This report documents the completion of Task 4A corrective actions addressing the three audit findings identified in `docs/audit/16-task4-security-verification.md`:
1. **SEC-T4-01 (Medium Severity): SSE Lifecycle Race and Premature Completion**:
   Resolved by replacing premature synchronous `mark_completed` invocation with a custom stream wrapper (`MonitoredStream`) providing a drop guard. The request transitions to `completed` **strictly** upon natural stream exhaustion (`Poll::Ready(None)`), while transport errors or early client disconnects trigger `mark_stream_interrupted`.
2. **SEC-T4-02 (Low Severity): Idempotency Index Hardening**:
   Resolved by introducing Migration 5 (`add_session_idempotency_unique_index`) and adding `CREATE UNIQUE INDEX IF NOT EXISTS idx_inference_requests_session_idempotency ON inference_requests(session_id, idempotency_key) WHERE session_id IS NOT NULL AND idempotency_key IS NOT NULL;`. Concurrent duplicate insertions for the same session idempotency key are rejected at the database engine level.
3. **SEC-T4-03 (Low Severity): Documentation Alignment**:
   Updated `docs/audit/15-inference-request-lifecycle-implementation.md` to accurately reflect Migration 4's true foreign key definitions (`ON DELETE SET NULL` on `session_id` and `reservation_id`, and `ON DELETE RESTRICT` on `payment_hash`).

All 152 test cases across 20 test suites pass clean; `cargo fmt` and `cargo clippy -D warnings` pass with 0 errors.

---

## 2. Exact Source Files Changed

1. **`src/node/api/handlers.rs`**:
   - Replaced premature `mark_completed` call before returning the Axum response with `MonitoredStream`.
   - `MonitoredStream` polls the underlying stream, transparently yielding SSE chunks.
   - On `Poll::Ready(Some(Err(_)))`: triggers `mark_stream_interrupted` with error reason `"stream_read_error"`.
   - On `Poll::Ready(None)`: triggers `mark_completed` with prompt token metrics.
   - On `Drop` before completion: triggers `mark_stream_interrupted` with error reason `"client_disconnected_early"`.
   - Uses atomic boolean flag (`completed: Arc<AtomicBool>`) to ensure mutual exclusion between completion and interruption.
2. **`src/node/storage/migrations.rs`**:
   - Added Migration 5: `add_session_idempotency_unique_index`.
   - Enforces unique index `idx_inference_requests_session_idempotency` on `(session_id, idempotency_key)` where both fields are not null.
3. **`src/node/storage/request.rs`**:
   - Cleaned up formatting on `InFlightRequestRow`.
4. **`tests/request_lifecycle_tests.rs`**:
   - Added `test_concurrent_duplicate_session_idempotency_keys_rejected` verifying that 8 racing tasks with the same session idempotency key yield exactly 1 winner and 7 unique constraint errors.
   - Added `test_sse_early_disconnect_marks_stream_interrupted_not_completed` verifying end-to-end through Axum routes that dropping an active stream before completion records `stream_interrupted` instead of `completed`.
5. **`docs/audit/15-inference-request-lifecycle-implementation.md`**:
   - Corrected schema documentation to match `migrations.rs` definitions.

---

## 3. Scope 1: Fix SEC-T4-01 — SSE Lifecycle Race

### Before vs. After Behavior

#### Before:
1. When streaming was requested, the handler connected to the upstream proxy.
2. As soon as the upstream response headers were verified, the handler constructed `monitored_stream` using `stream.inspect`.
3. In lines 398–404, `db.request_store().mark_completed(...)` was called **synchronously** before returning the HTTP response headers to Axum.
4. If a client disconnected or the stream encountered transport failure mid-generation, `stream.inspect` fired `mark_stream_interrupted`.
5. Because `mark_stream_interrupted` requires `WHERE status = 'processing'`, the conditional update failed (`rows_affected == 0`) since the row was already `completed`.
6. Result: An aborted stream was falsely recorded as `completed`.

#### After:
1. `MonitoredStream` wraps the underlying stream and implements `futures_util::Stream`.
2. When Axum polls the stream to write chunks to the client TCP socket:
   - Chunks are yielded transparently.
   - When the stream ends cleanly (`Poll::Ready(None)`), `self.completed.swap(true, Ordering::SeqCst)` atomically fires once, asynchronously calling `mark_completed(&req_id, Some(prompt_tokens), None)`.
   - If an error chunk occurs (`Poll::Ready(Some(Err(e)))`), `self.completed.store(true, Ordering::SeqCst)` fires once, asynchronously calling `mark_stream_interrupted(&req_id, "stream_read_error", None)`.
   - If the client drops the TCP connection or cancels the request mid-flight, Axum drops `MonitoredStream`. Its `Drop` implementation checks `!self.completed.load(Ordering::SeqCst)`. If not completed, it immediately spawns `mark_stream_interrupted(&req_id, "client_disconnected_early", None)`.
3. Because all updates use atomic SQL state filters (`status = 'processing'`), racing completion and interruption operations cannot both succeed, and the request cannot falsely remain in `completed` when dropped early.

---

## 4. Scope 2: Address SEC-T4-02 — Idempotency Index

### Schema Evolution: Migration 5
Added to `src/node/storage/migrations.rs`:
```sql
CREATE UNIQUE INDEX IF NOT EXISTS idx_inference_requests_session_idempotency
ON inference_requests(session_id, idempotency_key)
WHERE session_id IS NOT NULL AND idempotency_key IS NOT NULL;
```

### Analysis:
- **Null Safety**: Uses SQLite partial index clause `WHERE session_id IS NOT NULL AND idempotency_key IS NOT NULL`.
- **Payment Request Isolation**: Pay-per-request requests (`session_id IS NULL`) are unaffected by this index. Pay-per-request replay protection continues to be strictly governed by `payment_hash PRIMARY KEY` in `payment_authorizations`.
- **Requests without Idempotency Key**: Requests where `idempotency_key IS NULL` are omitted from the unique index, allowing multiple non-idempotent session queries.
- **Concurrent Racing Safety**: Verified with `test_concurrent_duplicate_session_idempotency_keys_rejected`. 8 concurrent tasks with the same session ID and idempotency key serialize at the SQLite level, resulting in 1 successful insert and 7 unique constraint errors.

---

## 5. Scope 3: Address SEC-T4-03 — Documentation Alignment

`docs/audit/15-inference-request-lifecycle-implementation.md` was updated:
- Changed foreign key actions for `session_id` and `reservation_id` from `ON DELETE RESTRICT` to `ON DELETE SET NULL`.
- Clarified that `ON DELETE SET NULL` ensures historical request auditing records are preserved even if ephemeral sessions are pruned.
- Added explicit documentation of `chk_auth_source` and `idx_inference_requests_session_idempotency`.

---

## 6. Scope 4: Regression Tests & Verification Results

### Dedicated New Tests in `tests/request_lifecycle_tests.rs`:
1. `test_concurrent_duplicate_session_idempotency_keys_rejected`:
   Spawns 8 concurrent tasks attempting to create an inference request with identical session ID and idempotency key. Asserts `success_count == 1` and `failure_count == 7`.
2. `test_sse_early_disconnect_marks_stream_interrupted_not_completed`:
   Sends a streaming request through full Axum router stack with WireMock returning an SSE stream. Drops the client response before reading the stream to EOF. Asserts that the persisted status in SQLite transitions to `stream_interrupted` (or `reconciliation_required` under crash recovery), and is **never** left as `completed`.

### Full Workspace Test Run:
```bash
cargo test --workspace
```
**Results:**
- `unittests (src/lib.rs)`: 40 passed
- `api_server_tests`: 10 passed
- `cli_tests`: 2 passed
- `client_budget_tests`: 3 passed
- `client_builder_tests`: 3 passed
- `client_integration_tests`: 5 passed
- `client_pay_tests`: 3 passed
- `config_tests`: 5 passed
- `discovery_tests`: 7 passed
- `gate_integration_tests`: 1 passed
- `lnd_backend_tests`: 7 passed
- `mock_lightning_tests`: 1 passed
- `nwc_backend_tests`: 6 passed
- `payment_authorization_tests`: 7 passed
- `privacy_audit_tests`: 1 passed
- `proxy_openai_tests`: 8 passed
- `request_lifecycle_tests`: **14 passed** (+2 new tests)
- `session_persistence_tests`: 16 passed
- `storage_tests`: 13 passed
- `unittests (src/main.rs)` & doc-tests: 0
**Total: 152 passed; 0 failed; 0 ignored.**

### Code Quality Verification:
- `cargo fmt --all -- --check`: Clean (exit code 0).
- `cargo clippy --all-targets --all-features -- -D warnings`: Clean (0 warnings, exit code 0).

---

## 7. Remaining Limitations

- In streaming mode, partial token emission is not parsed on the fly from Ollama chunk payloads. If an SSE stream is interrupted mid-generation, actual tokens generated prior to the drop are unknown and set to `None`.
- Phase 1 does not implement Hold Invoices or PayGuard (deferred to Phase 2).

---

## 8. Conclusion

All findings from `docs/audit/16-task4-security-verification.md` are resolved and verified. Task 4 is now complete, hardened, and ready for final review.

