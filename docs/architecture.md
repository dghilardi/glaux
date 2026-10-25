# Architecture

A single Rust package provides the `glaux` library and binary. The CLI and the MCP server call the same `execute` function, so they validate input and manage the session in the same way.

## Modules

- `config.rs`: paths, permissions, exclusive profile lock and Outlook host allowlist.
- `browser.rs`: isolated browser launch, CDP target selection, navigation and clean shutdown.
- `shutdown.rs`: SIGINT/SIGTERM registration before the browser starts (Unix).
- `cdp.rs`: sequential CDP websocket, response correlation and observation of OWA request headers. A context is accepted only from a successful response on the exact `/owa/service.svc` path; `requestWillBeSentExtraInfo` may arrive before or after the request and response.
- `owa.rs`: `FindItem`/`GetItem` payloads, in-page fetch transport and response normalization.
- `export.rs`: atomic, non-overwriting publication of private EML files; the MIME comes from the server.
- `archive.rs`: local classification rules, archive integrity, SQLite state, backfill cursor, explicit per-item exclusions and polling with backoff.
- `login.rs`: optional passkey sign-in; bounded actions on Microsoft sign-in hosts only, no secrets handled by Glaux.
- `guard.rs`: starts and stops a systemd authenticator unit based on the `loginctl` session state.
- `lib.rs`: input validation, authentication, the operation itself and cleanup on errors or Ctrl-C.
- `main.rs`: CLI, human/JSON output and diagnostics.
- `mcp.rs`: two read-only stdio tools with serialized calls.

## Lifecycle of a read

1. Validate input and acquire the profile lock.
2. Start Chromium with the dedicated profile and an ephemeral loopback-only CDP port. The browser sandbox stays enabled.
3. Keep one restored page (preferring Outlook) so that sessionStorage survives, close other pages of the Glaux profile and enable network events before navigating to Outlook.
4. Outlook resumes the saved session or shows the sign-in page. `login` always uses a visible window. With `--passkey-account`, Glaux can complete sign-in with an external passkey ([passkey](passkey.md)).
5. Observe an authenticated request to the OWA service on an allowed host. Bearer and canary values stay in memory and are never printed.
6. Run `fetch` inside the Outlook page with the browser's credentials. Page and endpoint must share the origin; API redirects are not followed.
7. Normalize the response: HTTP 200 does not automatically mean the OWA operation succeeded.
8. Close Chromium cleanly so cookies and storage are flushed, then release the lock.

HTTP 401/440 allows one renewal attempt by navigating to Outlook again. HTTP 403 is reported without retry loops, HTTP 429 is not retried automatically. Allowed operations are fixed in code and never derived from assistant input.

## Deliberate limits

Authentication depends on an external Chromium. Every command reloads the web app, which is slower than an HTTP client with its own refresh token, but keeps SSO and credential handling inside the browser and avoids depending on Microsoft's internal token flows.

Glaux does not use EWS SOAP, Microsoft Graph, the deprecated Outlook REST API or client IDs of other applications. The OWA payload version and UTC time zone are fixed by Glaux and not copied from the web app's own requests.

The EML archive is an explicit `sync`/`daemon` feature, separate from the browser profile. The daemon keeps its browser open while polling. There are no extra heartbeats to stretch the session and no application telemetry; Chromium and Outlook keep their own network behaviour.

Shutdown uses a separate CDP connection to the browser target, validated against the same loopback port, so it works even if the observed page has detached. A forced kill after a timeout prints a generic persistence warning.
