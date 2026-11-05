# Contributing to SatsGrid

Thank you for your interest in contributing to SatsGrid!

## Development Workflow

1. Ensure you have Rust (stable) and Cargo installed.
2. Clone the repository and install toolchain components:
   ```bash
   rustup component add rustfmt clippy
   ```
3. Run tests and linting before submitting pull requests:
   ```bash
   cargo test
   cargo clippy -- -D warnings
   cargo fmt --check
   ```

## Design Principles

- **Privacy First**: Never log prompt text or completions under any circumstance.
- **TDD (Test-Driven Development)**: Write tests verifying domain behavior alongside or before implementation.
- **Strict Error Handling**: Avoid unwrap or panic in production paths; use typed `Result` and `thiserror`.
