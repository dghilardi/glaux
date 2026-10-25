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
just check          # fmt, tests, clippy and the Python script tests, as in CI
just browser-test   # when browser/session code changes; needs Chromium
```

See [docs/testing.md](docs/testing.md). Add a regression test for every bug fix and update the documentation and the `Unreleased` section of [CHANGELOG.md](CHANGELOG.md) when behaviour changes. Do not claim support for a tenant or platform that has not passed [live validation](docs/live-validation.md).

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/).

## Releasing

Releases are published to [crates.io](https://crates.io/crates/glaux) by the `Release` workflow through crates.io trusted publishing; no registry token is stored in the repository.

Requirements: [just](https://github.com/casey/just) and [panrelease](https://github.com/dghilardi/panrelease).

```bash
just release patch   # or minor / major
```

The recipe requires a clean `main` in sync with `origin/main` and at least one entry under `Unreleased` in [CHANGELOG.md](CHANGELOG.md). It runs `just check`, lets panrelease bump `Cargo.toml`/`Cargo.lock`, move the `Unreleased` entries under the new version, commit and tag `vX.Y.Z`, verifies the package with `cargo publish --dry-run` and pushes `main` and the tag together. The workflow then repeats the checks, verifies that the tag matches the package version and publishes.

## License

By contributing, you agree that your contributions are dual licensed under the MIT and Apache-2.0 licenses, as described in the [README](README.md#license).
