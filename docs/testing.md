# Testing

All automated tests use synthetic data. None of them needs Microsoft credentials or touches a real mailbox.

```bash
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
python3 -B tests/test_validate_live.py
cargo test --locked --test browser_smoke -- --ignored
```

Use `RUST_TEST_THREADS=1 just browser-test` on resource-constrained desktops to avoid concurrent Chromium startup/session-restoration races.

- `cargo test`: OWA parsing and paging, application errors inside HTTP 200 responses, input validation, host allowlist, profile lock, CDP event ordering, archive integrity and exclusions, passkey driver limits, session guard decisions, and the CLI and MCP stdio processes (protocol negotiation, tool discovery, `isError` on invalid parameters) without opening an account.
- `tests/test_validate_live.py`: the live validation script against fake subprocesses, including cleanup on cancellation and timeouts. POSIX signal tests are skipped on Windows.
- Composition tests cover semantic HTML/recipient validation, Unicode, links/lists/tables, MIME alternatives generated from final HTML, preserved signature styling and image bytes, rejected active markup/attachments, write-policy enforcement and MCP discovery under both policies. No real email or signatures are used.
- Unix socket tests cover private endpoint ownership, stale sockets, symlinks and unsafe permissions, daemon policy enforcement, protocol/frame bounds, concurrent clients, queue overload, deadlines and response loss without retries. Real CLI/MCP subprocesses use a synthetic socket while a profile lock is held; stopping an MCP client disconnects its request without stopping the owner.
- `browser_smoke` (ignored by default, needs Chromium; one test needs a desktop session): profile persistence of local storage, persistent and session cookies and sessionStorage across restarts; clean shutdown even with a detached page; visible-browser startup; Ctrl-C during a delayed browser start; and the passkey driver walking synthetic sign-in pages on loopback and stopping at a password prompt. The draft OWA fixture also checks creation/update/readback, template immutability, stale versions, default write denial, saved-but-unverified results, and no write retries on HTTP 401/440/429 or lost responses. These tests use Glaux's own Rust/CDP launcher against a local HTTP server.

CI runs formatting, unit/integration tests, Clippy and the Python tests on every push and pull request. The browser tests and [live validation](live-validation.md) are run manually.

The shared-browser Chromium fixture interleaves archive MIME reads with concurrent socket read, draft creation and client-side export. It retains the same exclusive profile lock throughout and configures clients with an unusable browser path, so accidentally launching a second browser fails the test. All possible authentication renewals stay on loopback. Live daemon coexistence is not claimed by these fixtures.

The dependency versions locked in `Cargo.lock` declare Rust 1.88 as their highest minimum version; the project is developed with recent stable Rust.
