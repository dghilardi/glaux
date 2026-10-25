# Changelog

All notable changes to this project are documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Local archive reprocessing with dry runs, queued daemon requests and aggregate status reports.

- More specific archive classification with optional exact senders and separate example rules for monitoring, administration, development notifications and calendar messages.

- Opt-in per-category retention for the local archive (`[retention]` in the sync configuration); expired copies are not downloaded again.
- `just release patch|minor|major` to cut releases with panrelease.

## [0.1.0] - 2026-10-25

### Added

- Publishing to crates.io from CI on version tags.

- Optional unattended passkey sign-in (`--passkey-account`) and `passkey-guard` to run the authenticator only while the session is unlocked.
- Desktop notification when the daemon needs a new sign-in.
- Local EML archive with header-based categories, `sync` and a polling `daemon` with a systemd user unit.
- MIME export of single messages to EML.
- Live validation script for CLI and MCP that never prints mail content.
- Read-only CLI and MCP server for Microsoft 365 mail through Outlook on the web, with a persistent dedicated browser profile.
