# Roadmap

## Available

- Native Rust CLI with human and JSON output.
- Interactive Microsoft sign-in and MFA in a dedicated browser profile, persisted with session restore.
- One OWA renewal attempt on HTTP 401/440.
- Message listing with folder, limit and offset; reading with a plain-text body.
- Online status, local diagnostics and local session removal; one data directory per account.
- Cross-process profile lock and serialized MCP calls.
- MCP stdio server with `list_messages` and `read_message`.
- MIME export to private EML files and a local archive classified by header rules.
- Repeatable sync with a persisted cursor, integrity checks, polling with backoff and an optional systemd user unit.
- Optional unattended passkey sign-in with an external authenticator gated on the session lock.

## Next

1. **Broader validation:** session lifetime over days and reboots, other tenants, macOS and Windows.
2. **Folders and search:** folder enumeration and server-side search with reliable paging.
3. **Attachments:** verify MIME completeness on real messages; optional separate export with size limits and safe paths.
4. **Efficient sessions:** a local API so CLI/MCP can share the daemon's browser instead of competing for the lock.
5. **MCP over HTTP:** local authenticated transport with origin protection before any network exposure.
6. **Distribution:** release binaries and packages, multi-platform CI.

Sending, moving and deleting mail are out of scope for now. A Microsoft Graph backend could implement the same data model behind the same interface.
