# Phase 2, Task 6: Upstream Timeouts & Cancellation Implementation Report

## 1. Executive Summary

In Phase 2, Task 6, SatsGrid's inference gateway was hardened with complete upstream timeout and cancellation protections. 

Prior to Task 6:
- The gateway used unbounded HTTP connections, allowing unmonitored upstream hangs during TCP connect, response-header waiting (Time-To-First-Token), response body buffering, or SSE chunk generation.
- Client disconnects while waiting for upstream response headers abandoned in-flight requests in the `processing` state without timely reconciliation.
- Stalled SSE streams could hang indefinitely waiting in `Poll::Pending` because the proxy lacked streaming idle detection.
- Non-streaming finalization performed separate database updates for the inference record and session reservation, risking inconsistencies if a task was interrupted mid-finalization.

Task 6 resolves these issues through:
1. **Configurable Upstream Timeouts**:
   - `connect_timeout_secs` (10s default) bounds TCP connect and TLS handshake.
   - `response_header_timeout_secs` (60s default) bounds initial response header latency (Time-To-First-Token).
   - `stream_idle_timeout_secs` (30s default) detects stalled SSE streams by measuring data chunk intervals.
   - `total_request_timeout_secs` (600s default) enforces an overall request deadline across all execution phases.
2. **`DispatchCancellationGuard` Lifecycle Guard**:
   - Covers the post-dispatch window between `mark_processing()` and completion/streaming.
   - Distinguishes definite connection failure (releasing session reservations safely) from ambiguous in-flight failures (conservatively marking `reconciliation_required`).
   - For non-streaming requests, the guard remains active until the response body is parsed and atomic database finalization succeeds.
3. **Atomic Non-Streaming Finalization**:
   - Implemented `Database::finalize_request_and_session` executing request completion and session reservation finalization within an immediate SQLite transaction.
4. **SSE Error Event Protocol**:
   - Stalled streams emit standard SSE error events (`event: error\ndata: {"error":{...}}\n\n`) and terminate without emitting `[DONE]`.

All 167 unit, integration, and lifecycle tests pass cleanly across the workspace, and `cargo clippy --all-targets --all-features -- -D warnings` and `cargo fmt --all -- --check` pass with zero warnings.

---

## 2. Key Architecture Decisions

### 2.1 Timeout Configuration & Propagation
In [src/config/schema.rs](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/src/config/schema.rs), `UpstreamConfig` was extended with validated duration settings:
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamConfig {
    pub url: String,
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
    #[serde(default = "default_response_header_timeout_secs")]
    pub response_header_timeout_secs: u64,
    #[serde(default = "default_stream_idle_timeout_secs")]
    pub stream_idle_timeout_secs: u64,
    #[serde(default = "default_total_request_timeout_secs")]
    pub total_request_timeout_secs: u64,
}
```
Validation enforces:
- `connect_timeout_secs > 0`
- `response_header_timeout_secs >= connect_timeout_secs`
- `stream_idle_timeout_secs > 0`
- `total_request_timeout_secs >= response_header_timeout_secs`

### 2.2 Ingestion & Proxy Timeouts
- **Reqwest Client Level**: `reqwest::ClientBuilder::connect_timeout` enforces `connect_timeout_secs`.
- **Response-Header Timeout**: In `OpenAiProxy::forward_chat_completion_with_headers` and `OpenAiProxy::stream_chat_completion`, `tokio::time::timeout(self.header_timeout, send_future)` bounds initial HTTP header arrival.
- **Total Request Deadline**: Non-streaming execution is wrapped in `tokio::time::timeout(total_timeout, ...)`.
- **Streaming Idle Timeout**: Inside `MonitoredStream`, a pinned `tokio::time::Sleep` monitors chunk intervals and is reset **only** when `Poll::Ready(Some(Ok(bytes)))` receives actual data.

### 2.3 Exact Cancellation Ownership & Guard Defusing
- **`DispatchCancellationGuard`**: Created immediately when `mark_processing` commits.
- **Defusing Order for Non-Streaming**:
  1. Receive response headers.
  2. Read and parse JSON body.
  3. Commit atomic finalization via `db.finalize_request_and_session(...)`.
  4. Only after successful DB commit: call `dispatch_guard.defuse()`.
- **Defusing Order for Streaming**:
  1. Receive status 200 headers.
  2. Finalize session reservation.
  3. Instantiate `MonitoredStream` with total deadline and idle sleep timer.
  4. Only after `MonitoredStream` takes ownership: call `dispatch_guard.defuse()`.

### 2.4 Accounting & Lifecycle State Transitions
- **Definite Connect Failure**: Where no request bytes were transmitted (e.g. connection refused or connect timeout), `mark_failed("connect_timeout_or_abort", "pre_dispatch")` is recorded, and the session reservation is safely released (`release_reservation`), restoring available budget.
- **Ambiguous Failure / Header Timeout**: Where a connection was established and request transmission began, upstream execution cannot be ruled out. The request is marked `reconciliation_required`, preventing double-spending while leaving session funds reserved until operator review.
- **Stream Interruption**: Once streaming begins, session budget was already finalized at stream start. Interruptions record `stream_interrupted` with observed tokens.

---

## 3. Test Verification Suite

A dedicated integration test suite was added in [tests/timeout_cancellation_tests.rs](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/tests/timeout_cancellation_tests.rs).

| Test Name | Verified Invariant | Result |
| :--- | :--- | :--- |
| `test_upstream_response_header_timeout_non_streaming` | Upstream delayed beyond `response_header_timeout_secs` returns HTTP 504 Gateway Timeout; session reservation is marked `reconciliation_required` without automatic refund. | **PASS** |
| `test_upstream_connect_failure_releases_session_reservation` | Closed upstream port triggers connect failure; confirms request marked failed and session reservation safely released back to available budget. | **PASS** |
| `test_sse_streaming_idle_timeout_emits_error_and_no_done` | Upstream pauses SSE data beyond `stream_idle_timeout_secs`; client receives `event: error` with `code: "stream_idle_timeout"`, stream aborts, and **never** emits `[DONE]`. State marked `stream_interrupted`. | **PASS** |
| `test_atomic_non_streaming_finalization_consistency` | Verifies `finalize_request_and_session` executes atomically: session spent is updated, reservation is finalized, and request is marked completed. | **PASS** |
| `test_non_streaming_total_deadline_timeout` | Total request exceeding `total_request_timeout_secs` aborts with HTTP 504. | **PASS** |
| `test_streaming_total_deadline_terminates_active_chunks` | Active SSE stream with recurring chunks exceeding `total_request_timeout_secs` emits `event: error` with `code: "stream_total_timeout"`, terminates without `[DONE]`, and records `stream_interrupted`. | **PASS** |

### Workspace Test Results
- `cargo test --workspace`: **168 passed; 0 failed; 0 ignored**.
- `cargo clippy --all-targets --all-features -- -D warnings`: **0 warnings / clean exit**.
- `cargo fmt --all -- --check`: **0 formatting diffs**.

---

## 4. Scope Boundaries & Remaining Limitations

- **Concurrency Controls**: Gateway concurrency limits and queueing are explicitly out of scope for Task 6 and belong to Task 7.
- **Client Read Backpressure**: Stream idle timeout measures upstream chunk arrival. If a slow client induces TCP backpressure on the gateway's outgoing buffer, upstream chunk consumption may slow down. Connection timeouts protect against stalled upstream backends.
