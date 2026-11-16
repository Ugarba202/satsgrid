# Phase 2, Task 5: Request Validation & Body-Size Protection Implementation Report

## 1. Executive Summary

In Phase 2, Task 5, SatsGrid's HTTP request pipeline was hardened to enforce bounded payload ingestion and rigorous pre-authentication request validation. 

Prior to this task, the gateway accepted unstructured JSON payloads via Axum's standard `Json` extractor. Under that previous behavior:
1. Payloads were buffered according to default extractor policies without node-configured enforcement across streaming chunks or missing `Content-Length` headers.
2. The gateway issued L402 payment challenges or attempted credential verification before validating request parameters. Malformed requests could thus trigger payment challenges, and invalid requests submitted with valid credentials could debit session allowances or consume single-use payment hashes before validation errors were surfaced.

Task 5 resolves these issues by introducing:
- **`BoundedJson<T>` Extractor**: A streaming body reader that enforces `max_request_body_bytes` at the transport layer, immediately terminating chunked or Content-Length-advertised streams exceeding the limit and returning an OpenAI-compatible HTTP 413 response.
- **`ValidatedChatRequest`**: A modular, strongly-typed validator for OpenAI chat completions that validates model identifiers, messages, roles, tool call structures, and generation parameters (`temperature`, `top_p`, `presence_penalty`, `frequency_penalty`, `max_tokens` / `max_completion_tokens`).
- **Strict Pre-Authentication Ordering**: Syntactic and schema validation runs *prior* to L402 challenge creation, payment hash reservation, and session budget deduction. Invalid requests return HTTP 400 Bad Request with zero database side effects.

All 162 unit, integration, and lifecycle tests pass cleanly across the workspace, and `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` succeed with zero warnings.

---

## 2. Key Architecture & Design Decisions

### 2.1 Bounded Ingestion with `BoundedJson<T>`
- **Location**: [src/node/api/validation.rs](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/src/node/api/validation.rs)
- **Mechanism**:
  1. **Early Header Check**: Inspects `Content-Length` if present. If `Content-Length > max_request_body_bytes`, rejects the request immediately prior to reading the body stream.
  2. **Streaming Chunk Bounding**: When reading chunks from Axum's `Body`, tracks accumulated byte length. If `accumulated_len + chunk_len > max_request_body_bytes`, chunk ingestion terminates immediately, aborting further network transfer.
  3. **Structured OpenAI Error Formatting**: Overflow errors return HTTP 413 Payload Too Large with:
     ```json
     {
       "error": {
         "message": "Request body size exceeds maximum allowed size of 2097152 bytes",
         "type": "invalid_request_error",
         "param": null,
         "code": "request_too_large"
       }
     }
     ```
  4. **Syntax Parsing**: Malformed JSON bodies return HTTP 400 Bad Request with `code: "invalid_json"`.

### 2.2 Pre-Auth Request Validation Pipeline
- **Validation Rules**:
  - `model`: Non-empty string, length $\le 128$, characters restricted to ASCII alphanumeric, `.`, `-`, `_`, `/`, and `:`. No external network lookups are performed.
  - `messages`: Non-empty array, bounded to $\le 2048$ items.
  - `messages[i].role`: Supported roles are `"system"`, `"user"`, `"assistant"`, `"tool"`, `"function"`.
  - `messages[i].content`: Must be non-null unless role is `"assistant"` and contains `tool_calls` or `function_call`. Supports string content and array of content parts (`text`).
  - `messages[i].tool_call_id`: Required and non-empty when role is `"tool"`.
  - `max_tokens` / `max_completion_tokens`: Bounded positive integers. Per OpenAI specification, `max_completion_tokens` takes precedence when both are provided. Bounded by `max_completion_tokens_limit` (default: 128,000).
  - Floating Point Parameters:
    - `temperature`: Bounded in $[0.0, 2.0]$.
    - `top_p`: Bounded in $[0.0, 1.0]$.
    - `presence_penalty`: Bounded in $[-2.0, 2.0]$.
    - `frequency_penalty`: Bounded in $[-2.0, 2.0]$.
    - Rejects `NaN`, `Infinity`, null, and non-numeric types.
  - Preserved Fields: Pass-through fields (`tools`, `tool_choice`, `functions`, `function_call`, `response_format`, `stop`, `seed`, `user`) are retained in the request payload passed to upstream providers.

### 2.3 Strict Invariant Protection & Accounting Safety
The execution sequence in [src/node/api/handlers.rs](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/src/node/api/handlers.rs#L111) is structured as:
```
1. Bounded Body Read & JSON Deserialization (BoundedJson)
2. Schema & Parameter Validation (ValidatedChatRequest::validate)
   └── If invalid: return 400 Bad Request (Zero DB interaction)
3. Prompt & Completion Token Estimation & Cost Calculation
4. Authentication & Authorization Check
   ├── If unauthenticated: issue L402 challenge (HTTP 402)
   └── If authenticated: verify L402 macaroon & preimage
5. Two-Phase Reservation & Replay Prevention
   ├── Single-use: reserve_payment_hash in SQLite
   └── Session: reserve_budget in SQLite
6. Durable Dispatch & Upstream Execution
```
If an authenticated client submits a request with an out-of-bounds parameter or malformed message array:
- The request is rejected at Step 2.
- No `payment_authorizations` rows are inserted or marked consumed.
- No `session_reservations` rows are inserted.
- Session `remaining_sats` and `reserved_sats` remain unchanged.
- No `inference_requests` lifecycle records are created.
- The client retains their unspent payment preimage or session funds.

---

## 3. Configuration & Compatibility

In [src/config/schema.rs](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/src/config/schema.rs), `ServerConfig` was extended with backward-compatible defaults:
```rust
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub admin_token: Option<String>,
    #[serde(default = "default_max_request_body_bytes")]
    pub max_request_body_bytes: usize, // Default: 2,097,152 (2 MB)
    #[serde(default = "default_max_completion_tokens_limit")]
    pub max_completion_tokens_limit: usize, // Default: 128,000
}
```
Existing configuration files without these keys deserialize without errors.

---

## 4. Test Verification Suite

A dedicated integration test suite was added in [tests/request_validation_tests.rs](file:///home/mobile_dev/Desktop/Ucee/BossBattle/satsgrid/tests/request_validation_tests.rs).

| Test Name | Verified Behavior | Result |
| :--- | :--- | :--- |
| `test_request_body_exceeds_content_length_limit` | Rejects payload when `Content-Length > max_request_body_bytes` with HTTP 413 and `code: "request_too_large"`. | **PASS** |
| `test_chunked_streaming_request_exceeds_limit_without_content_length` | Streams chunks without `Content-Length`, terminates stream as soon as accumulated bytes exceed limit, returns HTTP 413. | **PASS** |
| `test_malformed_json_syntax_returns_400_openai_format` | Unparseable JSON returns HTTP 400 with `code: "invalid_json"`. | **PASS** |
| `test_pre_auth_validation_failure_returns_400_not_402` | Unauthenticated request missing required `messages` returns HTTP 400 Bad Request, NOT an L402 challenge. | **PASS** |
| `test_valid_request_without_auth_returns_402_challenge` | Syntactically and semantically valid request without auth returns HTTP 402 with `WWW-Authenticate` header. | **PASS** |
| `test_invalid_request_with_valid_payment_does_not_consume_payment_hash` | Authenticated pay-per-request with invalid parameter returns HTTP 400. Confirms 0 payment authorizations and 0 inference request records recorded; subsequent valid request succeeds with the same payment. | **PASS** |
| `test_invalid_request_with_valid_session_does_not_debit_budget` | Authenticated session request with invalid `max_tokens` returns HTTP 400. Confirms session balance is unchanged and 0 reservation records created. | **PASS** |
| `test_schema_model_validation` | Rejects empty model strings, models with invalid characters, and models exceeding 128 characters with HTTP 400. | **PASS** |
| `test_schema_messages_and_roles_validation` | Rejects empty messages array, unsupported roles (`"admin"`), null content for non-assistant roles, and tool messages missing `tool_call_id`. Permits assistant messages with null content when `tool_calls` are present. | **PASS** |
| `test_numeric_parameters_boundary_validation` | Validates bounds for `temperature`, `top_p`, `presence_penalty`, `max_tokens`, and verifies `max_completion_tokens` precedence over `max_tokens`. | **PASS** |

### Workspace Test Results
- `cargo test --workspace`: **162 passed; 0 failed; 0 ignored**.
- `cargo clippy --all-targets -- -D warnings`: **0 warnings / clean exit**.
- `cargo fmt --check`: **0 formatting diffs**.

---

## 5. Scope Boundaries & Remaining Limitations

- **Slowloris & Connection Timeouts**: Transport-level body size protection limits memory and stream size, but does not enforce connection or read timeouts. Inbound request read timeouts, upstream timeouts, and connection concurrency controls are explicitly designated for Task 6.
- **Multimodal Content**: Basic multi-part text arrays are supported for token estimation and schema validation. Full multimodal binary handling (e.g. image decoding/verification) is deferred until upstream image model support is integrated into the proxy.
