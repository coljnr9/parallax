# Parallax Project Rules

## Error Handling
- NEVER use `.unwrap()`, `.expect()`, `.unwrap_or()`, `.ok_or()`, etc.
- All error and option handling must be explicit using `match` or `if let`.
- Use the `?` operator only when the error type is correctly mapped to `ParallaxError`.

## Mandatory Quality Checks
Before considering any task complete, the following commands MUST be run and MUST pass with zero warnings/errors:
1. `cargo check`
2. `cargo clippy -- -D warnings -A clippy::upper_case_acronyms` (Allowing some common abbreviations if necessary, but enforcing strictness)
3. **Complexity Check**: `cargo clippy -- -D clippy::cognitive_complexity -D clippy::cyclomatic_complexity`

## Development Workflow
- Default to `async` for almost everything in the codebase.
- Mimic the style, structure, and architectural patterns of existing code.
- Always check `Cargo.toml` before assuming a library is available.
