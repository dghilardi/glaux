# Session and security

## What is stored

The dedicated profile holds what Chromium and Outlook normally store: cookies, storage, cache and SSO state. Glaux does not read personal browser profiles, never receives your password and never writes bearer tokens to a file. Tokens observed through CDP stay in process memory and are never placed in command-line arguments, output or logs.

On Unix the data and profile directories are created with `0700` and the lock file with `0600`. The browser manages the permissions and encryption of its internal files; whether the profile is encrypted depends on the browser, the OS and the keyring, not on Glaux. On Windows, place the data directory in a private location protected by user ACLs; Windows is not yet verified.

The CDP endpoint is a random loopback port that lives only for the duration of a command. Other local processes running as the same user could still reach it, so use the profile only in a trusted user session. The lock prevents two Glaux processes from using the same profile. Do not open the Glaux profile with a separate browser instance: an unclean shutdown can lose the saved session.

## Why sign-in can be requested again

Outlook can renew expired access tokens as long as the SSO session is valid. A revoked Entra session, a sign-in frequency policy or a mandatory MFA prompt is different: Glaux reopens the profile with session restore and, on HTTP 401/440, tries one extra navigation, but it does not change token lifetimes, mint credentials or bypass device or Conditional Access policies.

Persistent cookies reduce repeated sign-ins, but no fixed lifetime can be promised. Unmanaged devices are often subject to a daily sign-in frequency. No artificial activity is generated to keep a session alive.

The optional [passkey sign-in](passkey.md) lets Glaux repeat sign-in without interaction, using a dedicated passkey and an external authenticator that is active only while your session is unlocked. It does not bypass MFA or Conditional Access, but it replaces a human confirmation with the unlock of the local session. Whether that is acceptable is a decision for the account owner and their organization.

## Deletion and revocation

`glaux logout` takes the lock and deletes only `browser-profile` in the selected data directory. It does not delete mail or your personal browser profile and does not revoke the session on Microsoft's side: use your account or organization tools for remote revocation. It does not guarantee forensic deletion from disk.

## Mail and MCP

`export`, `sync` and `daemon` intentionally keep local EML copies, including attachments when present in the server's MIME. The archive is separate from the profile and is not removed by `logout`. Optional retention deletes local copies only; deletion is not secure erasure. SQLite holds OWA IDs, relative paths, checksums, receive times and expiry markers; `status.json` and the daemon journal contain only states and counts. Local rules read unverified headers.

The code issues only `FindItem` and `GetItem`. It never sends read-state updates, replies, deletions or messages. Outlook itself may still issue bootstrap, telemetry and sync requests. Reads request plain text, do not load remote images in the terminal and strip control and bidi characters from human output.

MCP exposes listing and reading only, never the browser, cookies, tokens, arbitrary URLs, shell or sign-in. Tool errors are flagged with `isError`. Email can contain prompt injection: treat returned content as untrusted data, never as instructions.

## Reporting problems

Never attach HAR files, browser profiles, CDP dumps or full OWA responses to issues. Reproduce problems with synthetic fixtures and identity-free error codes. Glaux deliberately omits JavaScript exception details and OWA error text, which could contain mailbox data. For vulnerabilities, see [SECURITY.md](../SECURITY.md).
