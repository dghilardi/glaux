# Glaux

**Your Microsoft 365 mailbox, from the terminal.** A native Rust CLI and local MCP server that read mail through Outlook on the web (OWA), using a persistent, isolated browser session.

Glaux is named after the Greek word for owl. It was inspired by [Owl for Exchange](https://addons.thunderbird.net/en-US/thunderbird/addon/owl-for-exchange/), but shares no code with it.

> [!WARNING]
> Glaux talks to undocumented OWA endpoints that Microsoft can change at any time. It is not affiliated with or endorsed by Microsoft. Your organization's sign-in, MFA, Conditional Access and acceptable-use policies still apply: check them before using Glaux with a work account.

## Status

Early prototype (0.1.0), tested on Linux with Chromium against one commercial Microsoft 365 tenant. It works there for listing, reading, MIME export, a local EML archive and MCP access. Other tenants, operating systems, Exchange on-premises and government clouds are untested.

Scope is deliberately **read-only**: Glaux only issues OWA `FindItem` and `GetItem`. It never sends, deletes, moves or marks messages as read.

## How it works

1. `glaux login` opens Chromium with a dedicated profile outside your personal browser. You sign in to Microsoft yourself, including MFA.
2. Glaux watches Outlook's own requests through the Chrome DevTools Protocol on loopback and keeps the authenticated request context in memory only.
3. Later commands reopen the same profile headless, let Outlook refresh its session through normal SSO, and issue read-only OWA calls from inside the page.

Glaux never sees your password and never writes tokens to disk. Session lifetime is decided by Microsoft and your tenant; see [session and security](docs/security.md).

## Requirements

- Rust 1.88 or newer.
- Chromium, Google Chrome or Microsoft Edge (`glaux doctor` shows which one is found; override with `--browser`).
- A graphical session for the first sign-in. Later commands run headless unless `--headed` is given.
- A work or school Microsoft 365 account with Outlook on the web enabled.

## Install

```bash
cargo install glaux --locked
glaux doctor
glaux login
glaux list --limit 10
```

`glaux login` succeeds only after a verification read of the inbox, then closes the browser so the profile is saved.

## Commands

```bash
glaux login                           # visible browser, waits up to 5 minutes
glaux status                          # online check: session + inbox read
glaux list --folder inbox --limit 20
glaux list --folder sentitems --limit 20 --offset 20
glaux read 'FULL_OWA_ITEM_ID'
glaux export 'FULL_OWA_ITEM_ID' --output /private/path/message.eml
glaux list --json | jq '.data.messages[] | {id, subject}'
glaux --headed status                 # for SSO flows that need a visible window
glaux doctor                          # local diagnostics, no sign-in
glaux logout                          # delete the local browser profile
glaux mcp                             # MCP server over stdio
```

- Folders: `inbox`, `sentitems`, `drafts`, `deleteditems`, `junkemail`, `archive`, or a known folder ID as `id:OWA_FOLDER_ID`. Folder listing is not implemented yet.
- IDs are opaque OWA ItemIds, not Graph IDs or `Message-ID` headers. Copy them from `list`.
- `read` requests a plain-text body and does not update `IsRead`. Outlook itself still loads in the browser and produces its normal traffic.
- `export` saves the server-provided MIME message, with attachments when the server includes them, atomically and without overwriting. The path must be absolute; the limit is 10 MiB per message.
- Output is JSON when piped (`{"ok":true,"schema_version":1,"data":...}`). Errors go to stderr with exit code 1.

## Configuration

Global flags (each also has an environment variable):

| Flag | Variable | Purpose |
|---|---|---|
| `--browser PATH` | `GLAUX_BROWSER` | Chromium-compatible executable |
| `--browser-platform auto\|x11\|wayland` | `GLAUX_BROWSER_PLATFORM` | Linux display backend for visible windows; `auto` prefers X11/XWayland when `DISPLAY` is set |
| `--data-dir PATH` | `GLAUX_DATA_DIR` | Private profile directory; use one per account |
| `--url URL` | `GLAUX_OWA_URL` | Start page, default `https://outlook.office.com/mail/` |
| `--passkey-account EMAIL` | `GLAUX_PASSKEY_ACCOUNT` | Optional unattended passkey sign-in, see [passkey](docs/passkey.md) |
| `--headed` | | Show the browser for every command |
| `--timeout SECONDS` | | Authentication wait, 1–900 |
| `--json` | | Force JSON output |

Allowed Outlook hosts are `outlook.office.com`, `outlook.office365.com` and `outlook.cloud.microsoft`, HTTPS only. The default data directory follows the platform convention (`~/.local/share/glaux` on Linux).

## MCP

Glaux exposes two read-only tools over stdio with the official Rust SDK: `list_messages` (`folder`, `limit`, `offset`) and `read_message` (`id`). It does not expose sign-in, sending, scripts or arbitrary requests. Run `glaux login` first, then configure your client:

```json
{
  "mcpServers": {
    "glaux": { "command": "/absolute/path/to/glaux", "args": ["mcp"] }
  }
}
```

Calls are serialized, and only one process can use a profile at a time. Mail content is untrusted input: messages can contain prompt-injection attempts, and the MCP client receives everything it reads.

## Local archive and daemon

```bash
glaux sync --config /absolute/path/sync.toml
glaux daemon --config /absolute/path/sync.toml --interval 300
```

`sync` downloads new messages of one remote folder as `messages/<category>/<sha256>.eml` under a private archive directory, with categories assigned by simple header rules ([sync.example.toml](sync.example.toml)). A SQLite state file tracks OWA IDs, checksums and a backfill cursor; `status.json` holds only state and counts. Files are published atomically and verified by checksum. They are kept forever unless you set a per-category `[retention]` such as `alerts = "10d"`: after each cycle, local copies older than that (by server receive time) are deleted and remembered, so they are not downloaded again. Retention never touches the server. Categories are local folders: nothing is moved on the server. Header rules do not prove sender identity. Rules are evaluated in order: automatic monitoring and administrative messages should precede project rules. An optional `sender` mailbox must match the parsed From address exactly (case-insensitive) **and** the `header`/`contains` condition must match. `content-type` also examines nested MIME parts, so it can identify calendar messages inside multipart mail. It does not examine text quoted in a body or descend into attached `message/*` messages. The example separates monitoring, administration, development notifications and calendar messages; unmatched mail stays in `other` for review. Categories do not enable embeddings or exclude messages from a consumer automatically.

`daemon` polls on an interval, keeps its browser open between successful cycles and backs off after errors, recording a content-free `reason` in `status.json`. On Linux a systemd user unit is provided in [packaging/systemd](packaging/systemd):

```bash
mkdir -p ~/.config/glaux ~/.config/systemd/user
cp -n sync.example.toml ~/.config/glaux/sync.toml
cp -n packaging/systemd/glaux.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now glaux.service
```

The daemon holds the profile lock: stop it before running `glaux login`, other online commands or MCP. Any EML consumer can index `<archive>/messages` without knowing Glaux.

## Reclassifying an existing archive

Edit the sync configuration, then ask the running daemon to preview or apply its rules:

```bash
glaux reprocess --config /absolute/path/sync.toml --dry-run
glaux reprocess --config /absolute/path/sync.toml --status
glaux reprocess --config /absolute/path/sync.toml
glaux reprocess --config /absolute/path/sync.toml --status
```

`reprocess` queues a private snapshot of the rules in the archive directory. The upgraded daemon processes it between sync cycles, including during network/authentication backoff, without opening another browser. During an idle wait it checks once per second; an active sync finishes first. A stopped daemon leaves the request queued until the next archive owner (`daemon` or `sync`) starts. Only one request can be pending. `--status` shows the pending request ID and the last `running`, `completed` or `failed` result; queuing is not confirmation of completion. Upgrade an older daemon before submitting requests.

A successful apply replaces the daemon's active classification rules for subsequent downloads. A dry run leaves them unchanged. Keep the updated rules in the daemon's own configuration file so they survive restarts. Requests carry rules only: polling, remote folder and retention settings are not reloaded. Changes to those settings require restarting the daemon.

For an offline archive, stop its owner and run `glaux reprocess --config /absolute/path/sync.toml --local` (optionally with `--dry-run`). This uses only the archive lock, without Chromium or Microsoft credentials. Use the same archive configuration as the daemon; omitting `--config` uses default settings, including an empty rules list.

Reprocessing verifies all tracked live files before moving them and reports only aggregate counts of scanned/moved EML files and destination categories. Identical messages sharing a file count once. It preserves MIME bytes, server receive times, backfill progress and expired-message tombstones. Files absent from SQLite are not imported. New copies are published before SQLite paths change, with durable cleanup records for interrupted moves. Failed operations can be retried; completed moves are retained. Consumers such as Librarian must reconcile changed paths and remove obsolete index entries.

Reprocessing never runs retention or restores expired copies. Later normal sync cycles apply the daemon's active retention to the new categories, so review retention before applying rules. The example leaves retention disabled and does not configure a Librarian index or embeddings.

## Development

```bash
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --test browser_smoke -- --ignored   # needs Chromium and a desktop session
python3 -B tests/test_validate_live.py
```

No automated test uses real credentials or mailbox data. See [testing](docs/testing.md), [architecture](docs/architecture.md), [live validation](docs/live-validation.md), [roadmap](docs/roadmap.md) and [CONTRIBUTING](CONTRIBUTING.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option. Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in Glaux, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
