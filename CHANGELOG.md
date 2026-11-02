# Changelog

All notable changes to this project are documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0] 2026-11-02

### Added

- Shared local mailbox service on Unix: CLI and MCP reuse the daemon's browser through a private socket while archiving continues, with bounded queues, daemon-enforced draft permissions and no automatic retries after uncertain writes.

## [0.4.0] 2026-11-02

### Added

- Local semantic HTML email previews with derived plain text, and opt-in CLI/MCP draft creation and updates using an Outlook signature-only draft, preserving supported Outlook formatting and inline images. Draft writes use `SaveOnly`, distinguish server conflicts from uncertain saves and are never retried automatically; saved MIME is checked by readback. Recipient-free create/update passed on one live session; broader compatibility remains experimental and sending remains disabled.

## [0.3.0] 2026-10-26

### Added

- Separate calendar invitations from responses and cancellations, and classify subscriptions using their unsubscribe header.

### Fixed

- Recognize calendars nested in multipart messages without classifying quoted headers or attached messages.

## [0.2.0] 2026-10-25

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
