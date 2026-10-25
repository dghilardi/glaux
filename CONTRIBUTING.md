# Contributing

Thanks for your interest in Glaux. Issues and pull requests are welcome.

## Ground rules

- **Never share real mailbox data.** No HAR files, browser profiles, CDP dumps, tokens, cookies, full OWA responses, message IDs, subjects or addresses, in issues, pull requests or test fixtures. Reduce problems to synthetic fixtures.
- **Read-only scope.** Glaux only lists and reads mail (`FindItem`, `GetItem`). Write operations need a design discussion in an issue first.
- **Respect tenant policies.** Do not add code that bypasses MFA, Conditional Access or device policies, fakes authenticators, or reuses client IDs of third-party Microsoft applications.
- Keep the browser sandbox, TLS, the exclusive profile lock and the loopback-only debugger intact.
- Keep protocol details in `src/owa.rs`; CLI and MCP must share the same core operations.

## Development

```bash
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
python3 -B tests/test_validate_live.py
cargo test --locked --test browser_smoke -- --ignored   # when browser/session code changes
```

See [docs/testing.md](docs/testing.md). Add a regression test for every bug fix and update the documentation and the `Unreleased` section of [CHANGELOG.md](CHANGELOG.md) when behaviour changes. Do not claim support for a tenant or platform that has not passed [live validation](docs/live-validation.md).

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/).

## Releasing

Releases are published to [crates.io](https://crates.io/crates/glaux) by the `Release` workflow through crates.io trusted publishing; no registry token is stored in the repository.

1. Move the `Unreleased` entries of [CHANGELOG.md](CHANGELOG.md) under the new version and bump `version` in `Cargo.toml`.
2. Merge to `main`, then tag and push: `git tag -a vX.Y.Z -m vX.Y.Z && git push origin vX.Y.Z`.
3. The workflow runs all checks, verifies that the tag matches the package version and publishes.

## License

By contributing, you agree that your contributions are dual licensed under the MIT and Apache-2.0 licenses, as described in the [README](README.md#license).
