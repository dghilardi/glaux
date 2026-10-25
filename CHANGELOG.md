# Changelog

All notable changes to this project are documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Optional unattended passkey sign-in (`--passkey-account`) and `passkey-guard` to run the authenticator only while the session is unlocked.
- Desktop notification when the daemon needs a new sign-in.
- Local EML archive with header-based categories, `sync` and a polling `daemon` with a systemd user unit.
- MIME export of single messages to EML.
- Live validation script for CLI and MCP that never prints mail content.
- Read-only CLI and MCP server for Microsoft 365 mail through Outlook on the web, with a persistent dedicated browser profile.
