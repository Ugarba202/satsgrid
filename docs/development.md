# Development & Contributing Guide

Thank you for contributing to SatsGrid! This document covers setup, architecture conventions, testing guidelines, and quality gates for developers.

---

## 1. Development Prerequisites

- **Rust Toolchain**: Stable (1.80+ or 2021 edition).
- **Cargo**: Standard package manager and build tool.
- **Git**: For version control.
- **Optional**: [Ollama](https://ollama.ai) for live upstream testing.

### Toolchain Verification
```bash
cargo --version
rustc --version
```

---

## 2. Codebase Architecture

```text
src/
├── bin/
│   └── main.rs             # CLI binary entry point
├── lib.rs                  # Library root and module exports
├── cli/                    # Clap CLI implementation (node, call, process)
├── client/                 # SatsGrid Caller SDK and budget tracking
├── common/                 # Domain types (Satoshis, Preimage, SessionId) & Errors
├── config/                 # Structured TOML configuration schemas
└── node/                   # SatsGrid Node server implementation
    ├── api/                # Axum HTTP routes and request handlers
    ├── gate/               # L402 challenge, macaroons, and budget manager
    ├── lightning/          # Lightning backend abstraction & Mock backend
    ├── pricing.rs          # Pricing engine & token count estimation
    ├── proxy/              # OpenAI proxy and upstream model discovery
    └── server.rs           # Node server runner and graceful shutdown
```

---

## 3. Strict Test-Driven Development (TDD)

All features, bug fixes, and security patches must include automated tests:

### Running the Test Suite
```bash
# Run all unit and integration tests
cargo test

# Run a specific test suite
cargo test --test api_server_tests
cargo test --test proxy_openai_tests
cargo test --test lnd_backend_tests
cargo test --test cli_tests
cargo test --test privacy_audit_tests
```

### Running Fast Unit Tests
```bash
cargo test --lib
```

---

## 4. Code Quality & Pre-Commit Gates

Before submitting a Pull Request, ensure your branch passes all automated quality gates:

### 1. Code Formatting
```bash
# Check formatting
cargo fmt --check

# Automatically format
cargo fmt
```

### 2. Static Analysis (Clippy)
Zero clippy warnings are permitted:
```bash
cargo clippy --all-targets -- -D warnings
```

### 3. Privacy Audit
Ensure zero prompt logging is maintained:
```bash
cargo test --test privacy_audit_tests
```

---

## 5. Pull Request Guidelines

1. **Branch Naming**: Use clear prefixes:
   - `feat/<feature-name>` for new capabilities
   - `fix/<bug-name>` for fixes
   - `docs/<doc-name>` for documentation updates
2. **Atomic Commits**: Keep commits focused and logically separated with conventional commit messages (`feat(...)`, `fix(...)`, `test(...)`, `docs(...)`).
3. **No Unrelated Files**: Avoid committing temporary test files, IDE configs, or external documents.
