# Unattended passkey sign-in

Optional. When the Microsoft session of the Glaux profile requires a new sign-in, Glaux can complete it without interaction using a dedicated passkey held by an external FIDO2 authenticator. Glaux never handles passwords, codes or keys: it enters the configured account, selects its tile, accepts "Stay signed in?" and waits for the authenticator to sign.

Enable it only if your organization's policies allow it. The decision belongs to the account owner.

## Security model

- The passkey is an additional credential, visible in the account's security info with a recognizable name (for example "Glaux – my-laptop"). You can revoke it there at any time.
- The tested authenticator is [passless](https://github.com/pando85/passless) 0.20.2 with its standard TPM 2.0 backend: the private key is encrypted with the machine's TPM and the file is useless elsewhere. While passless runs, the decrypted key is in its memory.
- In automatic mode (`PASSLESS_INTERACTION_MODE=automatic`) passless approves user presence and verification without a prompt. Any local process with access to the device can obtain a signature while the service runs.
- `glaux passkey-guard` limits that window: it starts the authenticator's systemd user unit only while the user's local graphical session is active, unlocked and not remote (`loginctl`: `Active=yes`, `LockedHint=no`, `Remote=no`), and stops it otherwise. If the state cannot be determined, it stops it. The state is polled every 2 seconds.
- passless's "portable" TPM backend (keys that never leave the TPM) uses a fixed persistent handle; if another object already occupies it, do not remove that object.

Residual risk: malicious code in the unlocked session can use the passkey and, with the standard backend, read the key from passless's memory. That is weaker than a non-exportable platform key such as Windows Hello.

## What Glaux does

With `--passkey-account EMAIL` (or `GLAUX_PASSKEY_ACCOUNT`), while waiting for an authenticated OWA request, Glaux inspects the sign-in page every second. It acts only on `login.microsoftonline.com` and `login.microsoft.com`, with at most one action per check, 3 seconds between actions and 6 actions in total:

- fill the account in the email field and continue;
- select the tile with exactly that account;
- accept "Stay signed in?";
- on the passkey page, wait for the authenticator.

If Microsoft asks for a password, shows an error or the flow does not progress, Glaux stops with an `authentication_or_protocol_required` error. The daemon then sends a content-free desktop notification (`notify-send`) on the first authentication failure and retries with its usual backoff. While the screen is locked the authenticator is stopped, so sign-in times out and succeeds on a later attempt after unlock.

## Setup on Linux with systemd

Requirements: passless with access to `/dev/uhid` (the `fido` group, effective after the next login), membership of the `tss` group for the TPM, and Chromium.

1. Run passless once in notification mode and register the passkey from any browser at https://mysignins.microsoft.com/security-info, choosing a USB security key. The passkey belongs to the account, not to a browser profile:

   ```bash
   install -d -m 700 ~/.local/share/passless/tpm
   passless --backend-type tpm --tpm-path ~/.local/share/passless/tpm --constant-signature-counter
   ```

2. Install the units, without enabling `passless.service` itself:

   ```bash
   mkdir -p ~/.config/systemd/user/passless.service.d ~/.config/systemd/user/glaux.service.d
   cp packaging/systemd/passless.service.d/glaux.conf ~/.config/systemd/user/passless.service.d/
   cp packaging/systemd/glaux-passkey-guard.service ~/.config/systemd/user/
   cp packaging/systemd/glaux.service.d/passkey.conf.example ~/.config/systemd/user/glaux.service.d/passkey.conf
   $EDITOR ~/.config/systemd/user/glaux.service.d/passkey.conf   # set your account
   systemctl --user daemon-reload
   systemctl --user enable --now glaux-passkey-guard.service
   systemctl --user restart glaux.service
   ```

`--constant-signature-counter` was needed on the test machine: each passless TPM operation took about 14 seconds, and updating the counter on every sign-in pushed the assertion past Chromium's limit. The trade-off is that the relying party cannot detect cloned passkeys through the counter. The daemon example uses `--timeout 180`.

## Disable and revoke

```bash
systemctl --user disable --now glaux-passkey-guard.service
rm ~/.config/systemd/user/glaux.service.d/passkey.conf
systemctl --user daemon-reload && systemctl --user restart glaux.service
```

Then delete the passkey from your Microsoft security info and, if no longer needed, `~/.local/share/passless/tpm`.

## Verified so far

On Arch Linux with KDE Plasma (Wayland), Chromium 155 and passless 0.20.2: Entra accepted the passkey registration; after the email, Microsoft opened the passkey page directly and requested that specific credential; a headless `glaux status` with a brand-new profile signed in without interaction in about 27 seconds and read the inbox; "Stay signed in?" was accepted and a second process reused the session; with the authenticator stopped the command failed cleanly at the timeout; the guard stopped passless within 2 seconds of locking the screen and restarted it on unlock. Daily renewal inside the daemon and behaviour after a reboot still need verification.
