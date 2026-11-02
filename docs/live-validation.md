# Validation with a real mailbox

These checks need the account owner for sign-in and MFA. Never give your password to an assistant or a script.

## First session

```bash
glaux doctor
glaux login
glaux list --limit 5
glaux read 'ID_FROM_LIST'
glaux status
```

Expected: sign-in in the dedicated window; success reported only after a valid inbox read; a list with IDs, subjects and senders; the body of the selected message. The read must not send an `IsRead` update. Also check `body_type` and that accented characters are preserved.

`login` succeeds only if OWA accepts `FindItem`. If web sign-in completes but Glaux reports a timeout or an unsupported response shape, the follow-up is protocol compatibility, not the password.

## Persistence

1. Close the Glaux browser and the terminal.
2. From a new terminal run `glaux status` and `glaux list --limit 5`.
3. Repeat after a reboot and on the next day, **without signing in again first**.
4. If headless fails, compare with `glaux --headed status` and complete any Microsoft prompts.

The presence of the profile directory is not proof of authentication. `doctor` is local; `status` performs an online read. A success on the same day proves reuse across processes, not a 24-hour Microsoft session.

For standalone persistence checks, stop the daemon first to release the profile lock. Normal Unix CLI/MCP calls instead share the running daemon's browser; these demonstrate service reuse, not independent browser/session restoration:

```bash
systemctl --user stop glaux.service
glaux status
python3 scripts/validate_live.py --glaux "$(command -v glaux)"
systemctl --user start glaux.service
```

Restart the service even if the check fails, then inspect the `reason` in `archive/status.json`.

## Shared daemon validation

Read-only coexistence passed on one existing live session; shared-service draft writes and broader tenant/platform coverage remain **pending**. Loopback Chromium tests verify shared browser ownership but do not establish live tenant behavior. After upgrading the daemon and clients, keep the daemon running and run `scripts/validate_live.py` normally. Confirm CLI and MCP list/read results match and archive progress continues, with no extra browser and no profile-lock errors. Use matching data directories and Outlook URLs. With explicit draft permission on both sides, create/update one synthetic recipient-free draft and check it in Outlook while archiving remains active. Test default service refusal of writes separately. Keep all IDs and content private; record only aggregate outcomes. Do not inject network failures into a real mailbox.

On 2026-11-02, Glaux 0.5.0 on Linux with Chromium 155.0.8059.84 passed CLI list/read/status and MCP discovery/list/read through the daemon socket. Two validation clients also passed simultaneously. The service PID remained unchanged across client calls, its browser profile lock stayed held, and archive health refreshed successfully after the upgrade. The socket was owned by the current user with mode 0600 in a 0700 directory. Daemon draft permission was enabled and confirmed in the effective service environment; no draft write was performed in this shared-service live check. No daemon stop/restart was needed between requests.

## Expected errors

- **No authenticated OWA request:** incomplete sign-in, policy, different endpoint or protocol change. Try `glaux --headed --timeout 120 status`.
- **HTTP 401/440:** one renewal was already attempted; sign in interactively again.
- **HTTP 403:** possibly a tenant policy or canary validation. A new sign-in is not guaranteed to fix it.
- **HTTP 429:** wait before retrying.
- **Unsupported OWA response:** needs an anonymized fixture, never a session dump.
- **Profile in use:** stop the CLI/MCP process or daemon using the same `--data-dir`.
- **Debugger unavailable when starting a visible browser:** on Linux try `glaux --browser-platform x11 login` (needs X11 or XWayland). `auto` already selects it when `DISPLAY` exists; a stall with native Wayland was observed with Chromium on KDE.
- **Origin mismatch:** page and observed endpoint are on different origins. Glaux stops without moving tokens between hosts; report the host combination, without credentials.

## MCP

After signing in from the terminal, connect `glaux mcp` with the default read-only policy to a local client. Check discovery of `list_messages`, `read_message` and `preview_email`, a short listing and reading a selected ID. Check a local preview without browser activity. Draft tools must be absent. The client receives the content it reads.

## Repeatable check without printing mail

From the repository root, after `glaux login`, with Python 3.11 or newer:

```bash
python3 scripts/validate_live.py --glaux /absolute/path/to/glaux
```

The script lists five inbox messages and reads the first one in separate CLI processes, then checks `status`. It starts a real MCP stdio server, negotiates the protocol, discovers the three read-only tools, calls `list_messages` and `read_message` and compares the message with the CLI result. It does not test draft writes. The inbox must contain at least one message. IDs and content stay in memory; the script prints only the outcome and saves no fixtures or logs. A message changing between the two reads can make the comparison fail: just repeat. Use the default policy without `GLAUX_MAIL_WRITE_POLICY=drafts` for this read-only script.

Use `--data-dir` if you signed in with a non-default directory; other `GLAUX_*` variables are inherited. The script accesses a real mailbox and is not part of `cargo test`. Signal-based cleanup is verified on Linux only.

## What to record

Date, operating system, browser version, OWA host and the outcome of login, list, read, MCP and session reuse after 24 hours and after a reboot. Never record tokens, cookies, message IDs, subjects or addresses.

## Drafts: opt-in manual validation

This checklist is **partially validated**; the full checklist remains incomplete. Synthetic loopback tests do not demonstrate OWA write compatibility on any live tenant. Run these checks only with the account owner's authorization to create and update test drafts. Do not enable sending or change installed services as part of validation.

1. Prepare a signature-only HTML draft in Outlook as described in [composition](composing.md). Include a Unicode signature, a link and a CID logo if applicable; leave all recipients empty. Wait for Outlook to save it. Keep its ID private and do not paste the content into issues.
2. Confirm default MCP discovery contains only the three read-only tools. A valid local preview must work without starting Chromium; invalid HTML/addresses must fail locally, including when the draft policy is enabled.
3. Configure a separate test invocation with `--mail-write-policy drafts`. Confirm five tools are exposed and no send tool exists. Create one draft from synthetic authored content and the template. Verify it appears only as an unsent draft; no sent item, move or read-state change is caused by Glaux.
4. Check subject, To/Cc/Bcc, Unicode, semantic formatting, links, signature styling and inline logo in Outlook. Confirm the template is unchanged. Inspect both stored MIME alternatives privately; verify `verified: true` corresponds to the actual saved content. A `verified: false` result must retain its saved ID and warn for manual review. Do not publish the MIME or message data.
5. Update the created draft with its current ChangeKey and confirm the same draft changes without a second signature or lost inline images. Change it in Outlook and verify a stale ChangeKey is rejected. Check server-side conflict rejection, not only the client's pre-read.
6. Verify the template cannot be updated through `update_draft`, a sent message is rejected, an addressed template is rejected, and drafts with additional attachments are refused without changes.
7. If a save is unconfirmed, inspect Drafts yourself before any retry. Use loopback tests, not a real mailbox, to inject network/authentication failures. Confirm normal shutdown leaves the dedicated profile reusable.

Record only date, platform/browser/host and aggregate outcomes: create, update, conflicts, signature, CID image, HTML/text preservation and template immutability. In particular, verify the tenant accepts these internal OWA MIME write payloads; EWS documentation is not proof. Broader tenant/platform support remains unverified until separately checked.

### Recorded validation, 2026-11-02

On Linux with Chromium 155.0.8059.84, one existing Microsoft 365 session passed CLI list/read/status reuse and MCP read-only discovery/list/read. The explicit draft-policy MCP invocation exposed five tools and no send tool. Creating one recipient-free test draft and updating the same item both returned `saved: true`, `is_draft: true` and `verified: true`. The stored MIME passed the HTML/text consistency check. The signature template's exported MIME remained byte-for-byte unchanged; stale ChangeKeys and attempts to update the template were rejected. The account owner confirmed the signature, logo, links and formatting looked correct in Outlook. The daemon was restored after each validation invocation.

The template used external HTTPS images, so live CID image preservation remains untested. Live recipient fields (including Bcc MIME behavior), server-side concurrent edits, sent/addressed/extra-attachment rejection, and persistence after a reboot or 24 hours remain pending. Server-side conflicts, CID images, attachment guards and uncertain saves are covered by synthetic loopback tests. This records one tenant/session only and does not establish broader support.

## Controlled email renderer validation

The inline renderer is covered by synthetic composition, MIME readback and CLI/MCP tests. A native Chromium fixture checks 375, 600 and 1024 px viewports without external resources. These checks do not establish live tenant or mail-client rendering compatibility.

On 2026-11-02, branch commit `642159e` was exercised against one real OWA session through a temporary daemon executable, then the installed 0.5.0 daemon was restored and confirmed active. One recipient-free draft with synthetic body text was created through MCP and returned `saved: true`, `verified: true`, `is_draft: true`. Subsequent read-only MIME checks confirmed empty recipient headers, no ordinary attachments, retained renderer width/font styles, matching signature text/links/image references and matching inline image bytes. The initial validation runner did not complete its strict template byte-identity check; template byte identity across the write is not established by this run. No template update or send operation was issued. Visual owner confirmation and other clients remain pending.

Before declaring the new presentation validated for a deployment:

- [x] Create a recipient-free test draft with the rendered body and an existing signature-only template; confirm saved MIME verification.
- [ ] Inspect headings, links, a small data table and wrapped long text in OWA.
- [ ] Confirm signature formatting and logo remain intact and the template remains unchanged.
- [ ] Inspect target desktop and mobile mail clients, including dark mode; record the exact clients checked. Do not infer support from Chromium screenshots.

Use synthetic body text for this validation and never record mailbox identifiers, addresses or message contents in this checklist. Sending mail requires separate explicit authorization.
