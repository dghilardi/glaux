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

If the daemon is running, stop it first to release the profile lock:

```bash
systemctl --user stop glaux.service
glaux status
python3 scripts/validate_live.py --glaux "$(command -v glaux)"
systemctl --user start glaux.service
```

Restart the service even if the check fails, then inspect the `reason` in `archive/status.json`.

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

After signing in from the terminal, connect `glaux mcp` to a local client. Check discovery of the two tools, a short listing and reading a selected ID. The client receives the content it reads.

## Repeatable check without printing mail

From the repository root, after `glaux login`, with Python 3.11 or newer:

```bash
python3 scripts/validate_live.py --glaux /absolute/path/to/glaux
```

The script lists five inbox messages and reads the first one in separate CLI processes, then checks `status`. It starts a real MCP stdio server, negotiates the protocol, discovers the two tools, calls `list_messages` and `read_message` and compares the message with the CLI result. The inbox must contain at least one message. IDs and content stay in memory; the script prints only the outcome and saves no fixtures or logs. A message changing between the two reads can make the comparison fail: just repeat.

Use `--data-dir` if you signed in with a non-default directory; other `GLAUX_*` variables are inherited. The script accesses a real mailbox and is not part of `cargo test`. Signal-based cleanup is verified on Linux only.

## What to record

Date, operating system, browser version, OWA host and the outcome of login, list, read, MCP and session reuse after 24 hours and after a reboot. Never record tokens, cookies, message IDs, subjects or addresses.
