# Testing

All automated tests use synthetic data. None of them needs Microsoft credentials or touches a real mailbox.

```bash
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
python3 -B tests/test_validate_live.py
cargo test --locked --test browser_smoke -- --ignored
```

- `cargo test`: OWA parsing and paging, application errors inside HTTP 200 responses, input validation, host allowlist, profile lock, CDP event ordering, archive integrity and exclusions, passkey driver limits, session guard decisions, and the CLI and MCP stdio processes (protocol negotiation, tool discovery, `isError` on invalid parameters) without opening an account.
- `tests/test_validate_live.py`: the live validation script against fake subprocesses, including cleanup on cancellation and timeouts. POSIX signal tests are skipped on Windows.
- `browser_smoke` (ignored by default, needs Chromium; one test needs a desktop session): profile persistence of local storage, persistent and session cookies and sessionStorage across restarts; clean shutdown even with a detached page; visible-browser startup; Ctrl-C during a delayed browser start; and the passkey driver walking synthetic sign-in pages on loopback and stopping at a password prompt. These tests use Glaux's own Rust/CDP launcher against a local HTTP server.

CI runs formatting, unit/integration tests, Clippy and the Python tests on every push and pull request. The browser tests and [live validation](live-validation.md) are run manually.

The dependency versions locked in `Cargo.lock` declare Rust 1.88 as their highest minimum version; the project is developed with recent stable Rust.
