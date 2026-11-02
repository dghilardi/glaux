# Architecture

A single Rust package provides the `glaux` library and binary. The CLI and the MCP server call the same `execute` function. It validates locally, returns offline previews immediately, and routes online operations to the local daemon when its socket exists. Without a daemon it retains standalone browser execution. `--standalone` explicitly bypasses routing but cannot take a profile owned by the daemon; `login` is always standalone.

## Modules

- `config.rs`: paths, permissions, exclusive profile lock and Outlook host allowlist.
- `browser.rs`: isolated browser launch, CDP target selection, navigation and clean shutdown.
- `shutdown.rs`: SIGINT/SIGTERM registration before the browser starts (Unix).
- `cdp.rs`: sequential CDP websocket, response correlation and observation of OWA request headers. A context is accepted only from a successful response on the exact `/owa/service.svc` path; `requestWillBeSentExtraInfo` may arrive before or after the request and response.
- `owa.rs`: `FindItem`/`GetItem` payloads, opt-in `SaveOnly` draft writes, in-page fetch transport and response normalization.
- `compose.rs`: local semantic HTML and recipient validation, normalization and plain-text derivation.
- `render.rs`: fixed local MJML/MRML layout, controlled accent, Outlook compatibility markup and an internally derived readback allowlist; no include loaders or remote resources.
- `draft.rs`: validated Outlook signature templates, related inline images, multipart alternatives and saved-MIME consistency checks.
- `export.rs`: atomic, non-overwriting publication of private EML files; the MIME comes from the server.
- `archive.rs`: local classification rules, archive integrity, SQLite state, backfill cursor, explicit per-item exclusions, opt-in retention and polling with backoff.
- `retention.rs`: retention periods and parsing of OWA receive times.
- `reprocess.rs`: private durable rule snapshots and aggregate results for reclassification by the archive owner; no browser or additional archive owner.
- `login.rs`: optional passkey sign-in; bounded actions on Microsoft sign-in hosts only, no secrets handled by Glaux.
- `guard.rs`: starts and stops a systemd authenticator unit based on the `loginctl` session state.
- `lib.rs`: shared validation and browser operations, local/daemon routing and standalone cleanup on errors or Ctrl-C.
- `service.rs`: bounded Unix socket transport, same-user peer checks, private endpoint ownership and the daemon's interactive queue. No browser handles or credentials cross this boundary.
- `main.rs`: CLI, human/JSON output and diagnostics.
- `mcp.rs`: three read-only stdio tools plus two draft tools exposed only under the process draft-write policy; calls are serialized.

## Lifecycle of a standalone read

1. Validate input and acquire the profile lock.
2. Start Chromium with the dedicated profile and an ephemeral loopback-only CDP port. The browser sandbox stays enabled.
3. Keep one restored page (preferring Outlook) so that sessionStorage survives, close other pages of the Glaux profile and enable network events before navigating to Outlook.
4. Outlook resumes the saved session or shows the sign-in page. `login` always uses a visible window. With `--passkey-account`, Glaux can complete sign-in with an external passkey ([passkey](passkey.md)).
5. Observe an authenticated request to the OWA service on an allowed host. Bearer and canary values stay in memory and are never printed.
6. Run `fetch` inside the Outlook page with the browser's credentials. Page and endpoint must share the origin; API redirects are not followed.
7. Normalize the response: HTTP 200 does not automatically mean the OWA operation succeeded.
8. Close Chromium cleanly so cookies and storage are flushed, then release the lock.

HTTP 401/440 allows one renewal attempt by navigating to Outlook again. HTTP 403 is reported without retry loops, HTTP 429 is not retried automatically. Allowed operations are fixed in code and never derived from assistant input.

The renewal retry applies to reads only. Draft writes are never retried. With `--mail-write-policy drafts`, creation uses `CreateItem`/`SaveOnly` in Drafts; updates require a current `IsDraft` and `ChangeKey`, with `UpdateItem`/`SaveOnly`/`NeverOverwrite`. Transport checks reject send dispositions and other mutation actions. The signature-only Outlook template is read with MIME and never modified. Unsupported signature markup or attachments stop the operation before a write. Authored HTML and signature become one HTML body, from which plain text is derived; CID images are copied into related MIME parts. A readback checks the saved content; failure to verify is reported separately from a successful save. Neither readback nor loopback fixtures establish live tenant support. See [composition](composing.md).

`preview_email` returns before profile locking or browser launch and excludes the online signature. The draft policy defaults to read-only and is supplied by CLI/environment configuration, never by tool parameters.

## Deliberate limits

Authentication depends on an external Chromium. Standalone commands reload the web app; daemon clients reuse its authenticated browser. SSO and credential handling stay inside Chromium, without depending on Microsoft's internal token flows.

Glaux does not use EWS SOAP, Microsoft Graph, the deprecated Outlook REST API or client IDs of other applications. The OWA payload version and UTC time zone are fixed by Glaux and not copied from the web app's own requests.

The EML archive is an explicit `sync`/`daemon` feature, separate from the browser profile. The daemon keeps its browser open while polling. It consumes local reprocessing requests between cycles and during backoff. Reclassification publishes verified copies before updating SQLite, records cleanup durably and preserves receive times and expired tombstones; it does not run retention. Successful requests also replace the active classification rules. There are no extra heartbeats to stretch the session and no application telemetry; Chromium and Outlook keep their own network behaviour.

## Shared mailbox service

On Unix the daemon binds `service.sock` in the dedicated data directory. The directory must be owned by the current user with mode 0700; the socket is 0600, symlinks are refused, and both peers verify the effective user ID. A separate lifetime lock prevents competing daemons, standalone commands, one-off sync and logout, including during browser failure backoff. Only an owned, private, refused stale socket can be replaced. Shutdown removes only the endpoint it created. Custom data directories must fit the platform's Unix socket path-length limit.

Each connection carries one versioned, length-prefixed JSON request and response. Requests contain a fixed operation and the configured Outlook URL, never arbitrary OWA actions, code, browser settings or write-policy overrides. Requests are capped at 2 MiB and responses at 16 MiB. At most 16 connections and 8 queued operations are retained. Handshake/response transmission is bounded to 10 seconds; clients and queued work have a 180-second deadline. Malformed data is rejected without echoing it or logging message data. Export transfers bounded MIME to the client, which validates and publishes its local output; the daemon does not accept output paths.

The archive loop remains the sole browser owner. It processes at most one live interactive request before each archive listing or MIME fetch, and serves requests during polling waits/backoff. An interactive operation, including a complete draft save/readback, is serialized against all other browser work. Both archive progress and interactive traffic get turns; a single OWA request or authentication can still delay a client. Policy and input checks run again in the daemon before execution. Drafts require permission in both the client process and daemon configuration.

Disconnected or expired requests are skipped if observed before execution. After execution starts, a client disconnect does not cancel a potentially applied write. Connection loss, timeout or interruption after submission never triggers a retry or standalone fallback; a write is reported as unconfirmed when its outcome is unknown. A stale or inaccessible existing endpoint is an error, not permission to launch a second browser. A missing endpoint falls back only when no daemon ownership lock is held. Client shutdown closes its own connection; daemon shutdown cleans up its owned browser and fails outstanding requests.

The Unix transport is verified with synthetic Linux fixtures. CLI/MCP reads also passed through a running daemon on one live Linux session, including concurrent clients. Recipient-free live draft creation and updates also passed through the shared service during MRML validation; other tenants/platforms require separate validation; see the [recorded checks](live-validation.md#shared-daemon-validation). Non-Unix platforms retain standalone operation.

Shutdown uses a separate CDP connection to the browser target, validated against the same loopback port, so it works even if the observed page has detached. A forced kill after a timeout prints a generic persistence warning.
