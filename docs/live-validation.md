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

Read-only coexistence passed on one existing live session. Recipient-free shared-service draft creation and updates subsequently passed during the [MRML renderer verification](#mrml-renderer-verification-2026-11-02); broader tenant/platform coverage remains **pending**. Loopback Chromium tests verify shared browser ownership but do not establish live tenant behavior. After upgrading the daemon and clients, keep the daemon running and run `scripts/validate_live.py` normally. Confirm CLI and MCP list/read results match and archive progress continues, with no extra browser and no profile-lock errors. Use matching data directories and Outlook URLs. With explicit draft permission on both sides, create/update one synthetic recipient-free draft and check it in Outlook while archiving remains active. Test default service refusal of writes separately. Keep all IDs and content private; record only aggregate outcomes. Do not inject network failures into a real mailbox.

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

On 2026-11-02, branch commit `642159e` was exercised against one real OWA session through a temporary daemon executable, then the installed 0.5.0 daemon was restored and confirmed active. One recipient-free draft with synthetic body text was created through MCP and returned `saved: true`, `verified: true`, `is_draft: true`. Subsequent read-only MIME checks confirmed empty recipient headers, no ordinary attachments, retained renderer width/font styles, matching signature text/links/image references and matching inline image bytes. The initial validation runner did not complete its strict template byte-identity check; template byte identity across the write is not established by this run. No template update or send operation was issued. The owner subsequently confirmed that headings, spacing, the table, links and the signature with its logo looked correct in Outlook. The specific Outlook variant was not recorded; this owner confirmation alone does not establish other clients or dark-mode behavior. External synthetic coverage is recorded below.

Before declaring the new presentation validated for a deployment:

- [x] Create a recipient-free test draft with the rendered body and an existing signature-only template; confirm saved MIME verification.
- [x] Owner confirms headings, spacing, links and a small data table look correct in Outlook.
- [x] Owner confirms signature formatting and logo look correct in the saved draft.
- [ ] Establish template byte identity across the renderer write.
- [ ] Inspect target desktop and mobile mail clients, including dark mode; record the exact clients checked. Do not infer support from Chromium screenshots.

Use synthetic body text for this validation and never record mailbox identifiers, addresses or message contents in this checklist. Sending mail requires separate explicit authorization.

### Initial inline renderer screenshot check, 2026-11-02

A single anonymous Unspam Device Preview test accepted the [synthetic HTML fixture](validation/email-renderer-2026-11-02.html) without an account, card or mailbox connection. The authored body was produced by branch commit `642159e`; its signature, name, organization, links and externally hosted placeholder logo were synthetic. No real message or signature was uploaded. This tests generated HTML directly, not MIME exported after Outlook saved a draft.

[Preview report](https://unspam.email/client-preview/50d2829f-2b88-42b2-ac10-36db340abffb). Anonymous results have limited retention; the committed HTML preserves the exact submitted artifact (SHA-256 `7c9a5dd3876a69afbb03927b17fcc6237f76b1bc5f6160f8f01662cfcf25cb7a`). The report initially listed 48 targets, then adjusted the total to 44; not every queued target had completed when the representative screenshots were reviewed. Do not treat the listed client count as a passed test count.

Screenshots were visually inspected for these provider-labelled targets:

| Target | Observation |
| --- | --- |
| iPhone 11 | Readable headings, bullets, table, wrapped link, separate code lines and synthetic signature/logo. |
| Android 9 | Readable body, table, wrapped link, separate code lines and synthetic signature/logo. |
| Gmail (Firefox) | Readable body with expected spacing, bullets, table, code lines and logo. |
| Outlook.com dark mode (Chrome) | Client inverts the palette; text, links and table remain visible in this sample. |
| Apple Mail dark mode | Body keeps a white background; content remains readable, but the two preformatted lines appear joined. |
| Microsoft Outlook 2016 / 2019 | Body/table extend beyond the intended maximum width, list markers are missing, table typography differs and preformatted lines appear joined. Content and logo remain visible. |

These are observed screenshots, not automated pixel comparisons or certification of the whole client family. These observations apply to the initial inline renderer. The subsequent MRML implementation and its separate checks are recorded below; do not treat this initial report as evidence for the later output. Arbitrary themes, complex tables, CID images and the actual corporate signature were not exercised by this external test.

### MRML renderer verification, 2026-11-02

Commit `8309a11` introduced a local MRML 6.0.1 layout while keeping authored semantic HTML and theme input unchanged. `just check` passed (67 Rust unit tests, 11 CLI/MCP/service integration tests and 7 Python tests); all eight native Chromium fixtures passed, including 375/600/1024 px layout checks with no remote resources and synthetic OWA creation/update/readback through the shared service. Regression checks cover reserved renderer markup, strict signature templates, malicious comment/stylesheet rejection, preformatted breaks and text alternatives without presentation-table borders.

A [first MRML screenshot run](https://unspam.email/client-preview/549d905a-29db-4b44-91e0-f865a99cd5d6) completed 44 provider-labelled targets after the owner signed in to Unspam's free account. The anonymous retry had been refused for quota and produced no report. Seven screenshots were inspected: Microsoft Outlook 2016/2019, Apple Mail dark mode, iPhone 11, Android 9, Gmail Firefox and Office 365 dark mode. In this sample, the constrained body width, missing list bullets and joined code lines observed with the initial renderer were corrected. The screenshots also exposed a signature outside the centered column.

Commit `706a006` moves the signature into a reserved internal row of the same MRML column and adds 16 px side padding. It preserves signature-specific formatting and supplies readable defaults rather than inheriting the layout's zero font size. Unit/MIME regression checks and all eight Chromium fixtures passed again. The original [first MRML HTML](validation/email-renderer-mrml-2026-11-02.html) remains as evidence for the first run; the corrected [final sample](validation/email-renderer-mrml-final-2026-11-02.html) was submitted to a [separate final screenshot run](https://unspam.email/client-preview/ddb65179-fbb9-49fc-8cc5-568fdb1c74d4). The final run completed all 44 provider-labelled targets. The final sample was visually inspected in eight provider-labelled screenshots: Microsoft Outlook 2016/2019, Apple Mail dark mode, iPhone 11, Android 9, Gmail Firefox, Office 365 dark mode and Outlook.com dark mode in Firefox. The sample has aligned body/signature, visible list markers, separate code lines, readable tables and a visible synthetic logo, without observed clipping or overlap. Apple Mail dark mode retains a white message background; the two Outlook dark-mode samples invert colors while keeping this sample readable. This is a representative visual review, not a pass assertion for every generated target. The owner subsequently confirmed in Outlook that the final MRML draft has correct body/signature/logo alignment, visible list markers, separate code lines and a readable table. The specific Outlook variant was not recorded. All externally uploaded body text, names, organization, links and logo were synthetic.

The temporary branch daemon created one recipient-free MRML draft using an existing signature-only template. Creation and subsequent updates of that same draft returned `saved: true`, `verified: true`, `is_draft: true` through MCP. The final update exercised `706a006`; the template's MIME and decoded parts were byte-identical across that update. Empty recipient fields were checked by the native saved-MIME comparison against the recipient-free input. No template update or send operation was issued. The installed 0.5.0 daemon was restored after each temporary test and confirmed active. An additional export-path check did not complete after the initial creation; it was not used as evidence for draft validity, and no creation was retried.

The final submitted HTML has SHA-256 `936adddfe287fb9916f864b2e77142d27f43c35a38e06f056fe45a39ee4e7aa9`.

The external service renders submitted synthetic HTML directly; it does not certify every transformation of an OWA-saved draft or every client/theme/table/signature combination. CID images remain covered by synthetic MIME fixtures, not this external logo test.

### Local theme configuration validation, 2026-11-03

Branch commit `e7d27c1` was exercised on Linux against one existing live OWA session using a temporary branch daemon. A persistent MCP client loaded the palette from the local data-directory `email-theme.toml`, without theme fields in the email request. The offline preview contained the configured text, border and decorative colors. Through the shared service, creation and an update of the same recipient-free draft both returned `saved: true`, `verified: true`, `is_draft: true`; a subsequent plain-text read confirmed the update. Native saved-MIME checks compared the recipient-free input, content, signature and images. The signature template's MIME remained byte-identical across both writes, and the temporary daemon PID stayed unchanged during the client calls. No send or template-update operation was issued.

The installed 0.6.0 service was restored and confirmed active after the test. The palette remains in local configuration only; no organization-specific colors, names or assets were added to the repository. `just check` passed with 69 Rust unit tests, 12 CLI/MCP/service integration tests and seven Python tests; all eight native Chromium fixtures passed, including responsive layout and shared-service draft writes using synthetic themes.

This establishes one live create/update and content-consistency run, not visual certification of the configured palette in Outlook or other mail clients. No new manual Outlook appearance confirmation or external cross-client screenshot run was performed for these theme options. Previously recorded rendering evidence remains limited to the samples and versions identified above.
