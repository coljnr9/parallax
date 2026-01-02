# Contributing to Parallax

Thank you for your interest in contributing to Parallax!

## Code of Conduct

This project and everyone participating in it is governed by the [Code of Conduct](CODE_OF_CONDUCT.md). By participating, you are expected to uphold this code.

## How Can I Contribute?

### Reporting Bugs
- Use the GitHub issue tracker.
- Describe the bug and provide steps to reproduce.
- Include information about your environment (OS, Rust version).

### Suggesting Enhancements
- Open an issue to discuss your idea before implementing it.

### Pull Requests
- Ensure the code follows the existing style.
- Run `cargo fmt` before committing.
- Ensure all tests pass: `cargo test`.
- Run clippy and ensure no warnings: `cargo clippy -- -D warnings`.

## Development Setup

1. Install Rust (latest stable).
2. Clone the repository.
3. Copy `.env.example` to `.env` and add your `OPENROUTER_API_KEY`.
4. Run `cargo build`.

## Coding Standards

- No `unwrap()` or `expect()`. Use explicit error handling.
- Errors should be mapped to `ParallaxError`.
- Use `async` for I/O bound operations.
- Follow the Hub & Spoke architecture defined in `specs/C2OR_SPEC.md`.

