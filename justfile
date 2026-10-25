# Glaux development tasks. Run `just` to list them.

default:
    @just --list

# Checks required by CI
check:
    python3 -B tests/test_validate_live.py
    cargo fmt --check
    cargo test --locked
    cargo clippy --locked --all-targets -- -D warnings

# Browser integration tests (need Chromium, no Microsoft account)
browser-test:
    cargo test --locked --test browser_smoke -- --ignored

# Release a new version: just release patch|minor|major
release level: _release-preflight check
    panrelease release {{level}}
    cargo publish --dry-run --locked
    git push --atomic origin main "v$(panrelease show)"

_release-preflight:
    #!/usr/bin/env bash
    set -euo pipefail
    test "$(git branch --show-current)" = main || { echo "Release from main only"; exit 1; }
    test -z "$(git status --porcelain)" || { echo "Working tree is not clean"; exit 1; }
    git fetch --quiet origin main
    test "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" || { echo "main is not in sync with origin/main"; exit 1; }
    awk '/^## \[Unreleased\]/{f=1;next} /^## \[/{f=0} f && /^- /{n++} END{exit n==0}' CHANGELOG.md || { echo "CHANGELOG.md has no Unreleased entries"; exit 1; }
