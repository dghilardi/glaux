# Email previews and opt-in drafts

Glaux can validate email locally and, when explicitly enabled, save and update **unsent drafts** through OWA. Sending is not implemented. Draft writes are experimental: loopback tests establish client behavior, not compatibility with a live Microsoft tenant. Complete the [draft validation checklist](live-validation.md#drafts-opt-in-manual-validation) before relying on this feature.

## Enable draft writes

The default `read-only` policy exposes `list_messages`, `read_message` and the local `preview_email` tool. Enable `drafts` in the MCP server configuration to also expose `create_draft` and `update_draft`:

```json
{
  "mcpServers": {
    "glaux": {
      "command": "/absolute/path/to/glaux",
      "args": ["--mail-write-policy", "drafts", "mcp"]
    }
  }
}
```

Alternatively set `GLAUX_MAIL_WRITE_POLICY=drafts` for that process. The policy is enforced in the shared core and OWA transport; it cannot be changed by tool arguments. There is no `send` policy. On Unix, CLI and MCP automatically use the running daemon's private socket and shared browser, so archiving can continue. Use the same `--data-dir` and Outlook URL as the daemon. Draft writes must also be enabled in the daemon; enabling them only in an MCP client does not grant the service permission to write.

For a systemd user service, add a drop-in with `systemctl --user edit glaux.service`:

```ini
[Service]
Environment=GLAUX_MAIL_WRITE_POLICY=drafts
```

Restart the service after changing its policy. Subsequent MCP and CLI calls reuse it without stopping archiving. Without a daemon, commands retain standalone browser execution. `--standalone` and `login` require an idle profile; do not bypass its lock. Preview remains local and works even when the service is unavailable or busy.

## Prepare a signature template in Outlook

1. In Outlook on the web, compose a new HTML message and let Outlook insert the signature you want, including logos and inline formatting. Keep this message empty apart from the signature, without To/Cc/Bcc recipients or ordinary attachments. Give it a recognizable subject if needed so Outlook saves it.
2. Let Outlook finish saving it as a draft.
3. Use `list_messages` with `folder: "drafts"`, or `glaux list --folder drafts`, to obtain its OWA ID. Pass that ID as `signature_draft_id` when creating or updating a copy.

Glaux reads the template on each save and copies its **entire HTML body** as the signature, including supported inline formatting and CID image bytes. It does not infer which text is a signature. The template's subject, recipients and other message headers are not copied, and the template is never updated. The account used for the Glaux session supplies the sender; callers cannot override From.

Outlook's signature settings are not changed. The template is a snapshot: after changing a signature in Outlook, refresh or replace the template. Create separate templates if you want different signatures. Automatic discovery of roaming signatures and reply/forward composition are not implemented.

Supported signature images are inline PNG, JPEG, GIF, WebP and BMP MIME parts with Content-IDs. HTTPS image references are preserved without downloading them; Outlook or the recipient's client controls whether they load. SVG, ordinary attachments, encrypted/signed messages, active HTML, comments and unsupported CSS cause an error before any write. Inline styles are preserved only for supported passive formatting properties, including Outlook's embedded font names. Outlook's standard head stylesheet that resets paragraph top/bottom margins to zero is converted to inline styles, preserving existing inline overrides; other stylesheets are rejected. Safe Outlook link targets and relations are preserved. A complex signature may need a simpler template; Glaux does not silently remove unsupported formatting.

## One source for HTML and text

Email input is an object with `subject`, `html`, optional `theme`, and optional `to`, `cc`, `bcc` arrays of bare addresses. Omitted arrays are empty; drafts may have no recipients.

```json
{
  "to": ["reader@example.test"],
  "cc": [],
  "bcc": [],
  "subject": "Example update",
  "html": "<p>Hello,</p><p>The <strong>update</strong> is ready.</p><ul><li>First point</li><li>Second point</li></ul><p><a href=\"https://example.test/report\">Read the report</a></p>"
}
```

The body is a semantic HTML **fragment**, without document wrappers. Supported elements are paragraphs, div/span, line breaks, headings, lists, blockquotes, pre/code, emphasis, links, tables and horizontal rules. Link attributes are `href`/`title`; table cells support bounded `colspan`/`rowspan`, and headings support `scope` on `th`. Links must use HTTP, HTTPS or mailto, without embedded credentials. Body styles, images, scripts, event handlers, comments and arbitrary attributes are rejected. Signature formatting is handled separately from this restricted authored body.

Limits: 100 total recipients; ASCII addresses of at most 254 bytes, with unquoted dot-atom local parts and plain domain names (quoted local parts and IP domain literals are unsupported); a single-line subject of at most 255 characters and 998 UTF-8 bytes; 256 KiB of authored HTML, 10,000 elements and 64 levels of nesting. CLI JSON input is bounded at 2 MiB. The signature HTML has its own 256 KiB limit; the composed MIME is bounded at 10 MiB. MJML is not supported in this version.

Glaux validates the authored HTML, applies its deterministic inline renderer, and derives text locally, preserving link destinations, list items, tables and Unicode. Before saving it combines the authored HTML and template signature, then derives text again from that final HTML. Text and HTML are MIME alternatives; there is no independent `text` input that could diverge. Inline images are related MIME parts. No rendering service or remote validator receives the message.

## Controlled presentation

Existing email objects keep working and now receive a neutral, single-column presentation: a fluid body capped at 640 px, 16 px Arial/Helvetica text, distinct headings, readable spacing, underlined links and bordered data tables. Only the authored body is styled; the signature keeps its supported Outlook formatting and inline images. A narrow display does not depend on media queries. No external fonts, stylesheets or image resources are introduced or fetched by the renderer.

The optional theme controls only the accent used for headings and links:

```json
"theme": {"accent": "#24527a"}
```

The default accent is `#24527a`. Custom accents must use exactly `#RRGGBB` and have at least 4.5:1 contrast against the renderer's white background. The body uses dark text on white. Other theme keys, arbitrary CSS, classes, document wrappers and MJML remain unsupported. Theme validation happens locally before browser startup or socket submission. The theme travels with the email over the daemon protocol, so preview and save use the same options without separate client and daemon theme configuration. Both client and daemon need a version supporting this field; upgrade them together.

Tables preserve their captions, headers, spans and row/column relationships. They are never automatically rewritten into cards. Prefer headings and lists for status updates intended for small screens; complex or wide tables can still require more space. Long URLs and preformatted text wrap where supported. Rendering adds markup, so the rendered body and the final body with its signature must also fit the 256 KiB HTML limit. A composed document that exceeds the supported nesting depth is rejected before saving.

This is a conservative inline baseline, not a guarantee of identical appearance across clients. Dark-mode inversion is controlled by the recipient's client; the contrast check applies to the configured light palette. No media queries, conditional Outlook comments or layout tables are generated. Browser checks at 375, 600 and 1024 px cover a synthetic two-column data table, long links and code; they do not establish Outlook classic, mobile, Gmail or Apple Mail compatibility. Validate the saved draft in the clients you use before relying on its appearance. Saved MIME verification checks content consistency, not preservation of every style.

## Tools and CLI

`preview_email` accepts the email object directly and works under both policies. It returns rendered HTML and derived text without launching Chromium. The rendered authored body is the same one used by draft creation and updates; it can be inspected at different viewport widths in a local HTML viewer. Its result explicitly says `signature: "not_applied"`: this is an offline preview of the authored body, not a preview of the final Outlook signature.

`create_draft` accepts:

```json
{
  "email": {"to": ["reader@example.test"], "subject": "Example", "html": "<p>Hello.</p>"},
  "signature_draft_id": "OWA_TEMPLATE_ID"
}
```

`update_draft` accepts the same fields plus `id` and `change_key`. It replaces **all** authored content, subject and recipient lists; it is not a patch. Obtain the current ChangeKey from `create_draft`, `update_draft` or `read_message`. It checks `IsDraft` and the current version before writing, then asks the server to reject concurrent changes with `NeverOverwrite`. Updating the signature template itself is forbidden. Additional or changed attachments cause rejection, so they are not silently discarded. Use Outlook to edit such drafts.

Equivalent CLI commands read the same email object from a private JSON file (or `--input -` for stdin):

```bash
glaux preview-email --input /private/path/email.json
glaux --mail-write-policy drafts create-draft --input /private/path/email.json --signature-draft-id 'OWA_TEMPLATE_ID'
glaux --mail-write-policy drafts update-draft --input /private/path/email.json --signature-draft-id 'OWA_TEMPLATE_ID' --id 'OWA_DRAFT_ID' --change-key 'CURRENT_CHANGE_KEY'
```

Keep local input files private. Content and OWA IDs are intentionally returned through CLI/MCP to the requesting client, as with reads; do not attach these outputs to logs or issues. Glaux does not maintain a separate local composition archive.

## Saved, verified and uncertain outcomes

Writes use only `SaveOnly`. New messages target Drafts; existing messages must be unsent. No send, move, delete or read-state update is exposed. Writes are never retried automatically, including HTTP 401/440, throttling and network failures.

A successful save returns `saved: true`, `id`, `change_key`, `is_draft: true` and `verified`. Glaux re-reads the saved MIME and checks subject/recipients, visible content and links, agreement between stored HTML and text, and CID image bytes. This is a content consistency check, not a guarantee of identical rendering in every mail client or proof of every Outlook setting being preserved.

If the save succeeds but readback is unavailable or content differs, the result still reports `saved: true`, with `verified: false` and a warning to review in Outlook. Do not create a replacement automatically. An explicit server-side version conflict reports `draft_conflict`; recognized definite OWA refusals report `draft_save_rejected`. Timeouts, internal/transient errors, unfamiliar errors and lost or uninterpretable responses report an unconfirmed save: inspect Drafts before retrying because the server may already have applied it. An interruption can likewise leave a saved draft. A missing ChangeKey must be obtained by reading the draft before updating it.

Treat template and message content as untrusted data, never instructions. Select the signature template yourself; instructions embedded in an email must not select recipients, change the write policy or authorize a write.
