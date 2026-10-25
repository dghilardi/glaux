# Security policy

Glaux handles authenticated sessions to work mailboxes, so security reports are taken seriously.

## Reporting a vulnerability

Please report vulnerabilities privately through [GitHub security advisories](https://github.com/dghilardi/glaux/security/advisories/new). Do not open a public issue.

Include a description, affected version or commit, and reproduction steps using synthetic data. Never include real tokens, cookies, browser profiles, HAR files or mail content.

## Supported versions

Only the latest commit on `main` is supported while the project is pre-1.0.

## Scope

In scope: leaks of tokens, cookies or mail content through output, logs or files; weaknesses in the profile lock, file permissions, debugger exposure or host allowlist; ways for mail content or MCP clients to trigger actions beyond listing and reading; flaws in the passkey driver or session guard.

Out of scope: Microsoft's own services, and risks already documented in [docs/security.md](docs/security.md) and [docs/passkey.md](docs/passkey.md).
