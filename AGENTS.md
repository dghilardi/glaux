# Glaux development

This is a native Rust CLI/library with an OWA backend and a local MCP stdio server.

- Keep OWA operations limited to listing and reading unless the user explicitly expands scope.
- Do not inspect personal browser profiles. Session state belongs only in the dedicated Glaux profile outside this repository.
- Never log cookies, bearer tokens, CDP event payloads, HAR files, full OWA errors or real mail fixtures. Use synthetic test data.
- Preserve the exclusive profile lock, browser sandbox and loopback-only debugger. Do not add client IDs belonging to third-party Microsoft applications.
- Login/MFA is completed by the user in Chromium, unless the user opted into passkey sign-in (`docs/passkey.md`): then Glaux only enters the account, picks its tile, accepts "Stay signed in?" and waits for an external authenticator. Never type passwords or codes, never fake authenticators. Respect tenant policies and distinguish local profile persistence from proven Microsoft session duration.
- Validate inputs before launching a browser; keep CLI stdout clean for JSON/MCP.
- Keep protocol details in `src/owa.rs`; CLI and MCP must share the same core operations.
- For browser integration tests, use this repository's Rust/CDP launcher against local fixtures. Run `cargo test --locked --test browser_smoke -- --ignored`; it needs Chromium and no Microsoft credentials.
- Required checks: `cargo fmt --check`, `cargo test --locked`, `cargo clippy --locked --all-targets -- -D warnings`, `python3 -B tests/test_validate_live.py`. Run the browser smoke test when browser/session code changes.
- Do not claim live tenant support until the checklist in `docs/live-validation.md` has actually passed. Update docs and the `Unreleased` section of `CHANGELOG.md` when behavior changes.
