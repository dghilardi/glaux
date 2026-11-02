//! Copy a signature-only Outlook draft without copying its envelope or unrelated attachments.
use crate::compose::{MAX_HTML_BYTES, PreparedEmail, plain_text};
use anyhow::{Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use mailparse::{DispositionType, MailHeaderMap, ParsedMail};
use scraper::{ElementRef, Html, Node, Selector};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const MAX_MIME_BYTES: usize = 10 * 1024 * 1024;

#[derive(PartialEq, Eq)]
struct InlineImage {
    mime_type: String,
    bytes: Vec<u8>,
}

pub(crate) struct Signature {
    html: String,
    images: BTreeMap<String, InlineImage>,
}

impl Signature {
    pub fn from_mime(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= MAX_MIME_BYTES,
            "signature_template_too_large"
        );
        let mail =
            mailparse::parse_mail(bytes).map_err(|_| anyhow::anyhow!("invalid_template_mime"))?;
        ensure!(
            mail.parts()
                .filter(|part| part.ctype.mimetype == "text/plain")
                .count()
                <= 1,
            "draft_has_attachments: multiple text parts are not supported"
        );
        let mut html = None;
        let mut images = BTreeMap::new();
        collect(&mail, &mut html, &mut images, 0)?;
        let html = html.ok_or_else(|| {
            anyhow::anyhow!("signature_template_requires_html: create an HTML draft in Outlook")
        })?;
        ensure!(html.len() <= MAX_HTML_BYTES, "signature_template_too_large");
        let document = Html::parse_document(&html);
        validate_signature(&document, &images)?;
        let body = document
            .select(&Selector::parse("body").unwrap())
            .next()
            .ok_or_else(|| anyhow::anyhow!("invalid_signature_html"))?;
        // Outlook uses inline formatting. Head styles cannot be preserved by this first version.
        Ok(Self {
            html: wrap_signature(
                document.root_element(),
                &wrap_signature(body, &body.inner_html()),
            ),
            images,
        })
    }

    /// Refuse to discard attachments added independently in Outlook.
    pub fn check_existing_images(&self, existing: &[u8]) -> Result<()> {
        let current = Self::from_mime(existing)?;
        ensure!(
            self.images == current.images,
            "draft_attachments_changed: inline images differ from the signature template; edit this draft in Outlook"
        );
        Ok(())
    }

    pub fn mime(&self, email: &PreparedEmail) -> Result<Vec<u8>> {
        let html = format!(
            "<html><body><div>{}</div><div>{}</div></body></html>",
            email.html, self.html
        );
        let text = plain_text(&html)?;
        let digest = format!("{:x}", Sha256::digest(html.as_bytes()));
        // RFC 2046 limits boundary values to 70 characters, including our prefix.
        let hash = &digest[..40];
        let alternative = format!("glaux_alt_{hash}");
        let related = format!("glaux_related_{hash}");
        let mut mime = String::from("MIME-Version: 1.0\r\n");
        mime.push_str("Subject: ");
        let mut word = String::new();
        let mut words = Vec::new();
        for ch in email.subject.chars() {
            if word.len() + ch.len_utf8() > 42 {
                words.push(format!("=?UTF-8?B?{}?=", STANDARD.encode(word.as_bytes())));
                word.clear();
            }
            word.push(ch);
        }
        if !word.is_empty() {
            words.push(format!("=?UTF-8?B?{}?=", STANDARD.encode(word.as_bytes())));
        }
        mime.push_str(&words.join("\r\n "));
        mime.push_str("\r\n");
        for (name, addresses) in [("To", &email.to), ("Cc", &email.cc), ("Bcc", &email.bcc)] {
            if !addresses.is_empty() {
                mime.push_str(&format!("{name}: {}\r\n", addresses.join(",\r\n ")));
            }
        }
        if !self.images.is_empty() {
            mime.push_str(&format!(
                "Content-Type: multipart/related; boundary=\"{related}\"\r\n\r\n--{related}\r\n"
            ));
        }
        mime.push_str(&format!(
            "Content-Type: multipart/alternative; boundary=\"{alternative}\"\r\n\r\n"
        ));
        for (kind, body) in [("plain", text.as_bytes()), ("html", html.as_bytes())] {
            mime.push_str(&format!("--{alternative}\r\nContent-Type: text/{kind}; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n"));
            encode_body(&mut mime, body);
        }
        mime.push_str(&format!("--{alternative}--\r\n"));
        for (cid, image) in &self.images {
            mime.push_str(&format!("--{related}\r\nContent-Type: {}\r\nContent-ID: <{cid}>\r\nContent-Disposition: inline\r\nContent-Transfer-Encoding: base64\r\n\r\n", image.mime_type));
            encode_body(&mut mime, &image.bytes);
        }
        if !self.images.is_empty() {
            mime.push_str(&format!("--{related}--\r\n"));
        }
        ensure!(mime.len() <= MAX_MIME_BYTES, "composed_message_too_large");
        Ok(mime.into_bytes())
    }
}

// Preserve inherited formatting/direction on the source html/body without embedding
// document elements inside the composed body. Attributes have already been validated.
fn wrap_signature(element: ElementRef<'_>, contents: &str) -> String {
    let mut wrapper = String::from("<div");
    for (name, value) in element.value().attrs() {
        let escaped = value
            .replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        wrapper.push_str(&format!(" {name}=\"{escaped}\""));
    }
    wrapper.push('>');
    wrapper.push_str(contents);
    wrapper.push_str("</div>");
    wrapper
}

fn encode_body(target: &mut String, bytes: &[u8]) {
    let encoded = STANDARD.encode(bytes);
    for chunk in encoded.as_bytes().chunks(76) {
        target.push_str(std::str::from_utf8(chunk).unwrap());
        target.push_str("\r\n");
    }
}

/// Exchange may normalize MIME. Compare envelope, visible content, links and image bytes,
/// and require the stored plain-text alternative to agree with the stored HTML.
pub(crate) fn verify_saved(expected: &[u8], actual: &[u8]) -> Result<()> {
    let expected_signature = Signature::from_mime(expected)?;
    expected_signature.check_existing_images(actual)?;
    let expected_mail =
        mailparse::parse_mail(expected).map_err(|_| anyhow::anyhow!("invalid_composed_mime"))?;
    let actual_mail =
        mailparse::parse_mail(actual).map_err(|_| anyhow::anyhow!("invalid_saved_mime"))?;
    ensure!(
        expected_mail.headers.get_first_value("Subject")
            == actual_mail.headers.get_first_value("Subject"),
        "saved_subject_mismatch"
    );
    for field in ["To", "Cc", "Bcc"] {
        let addresses = |mail: &ParsedMail<'_>| -> Result<Vec<String>> {
            let value = mail.headers.get_first_value(field).unwrap_or_default();
            let addresses = mailparse::addrparse(&value)
                .map_err(|_| anyhow::anyhow!("invalid_saved_recipients"))?;
            let mut result = Vec::new();
            for address in addresses.iter() {
                match address {
                    mailparse::MailAddr::Single(address) => {
                        result.push(address.addr.to_ascii_lowercase())
                    }
                    _ => bail!("unsupported_saved_recipients"),
                }
            }
            result.sort();
            Ok(result)
        };
        ensure!(
            addresses(&expected_mail)? == addresses(&actual_mail)?,
            "saved_recipients_mismatch"
        );
    }
    fn plain_parts(mail: &ParsedMail<'_>, parts: &mut Vec<String>) -> Result<()> {
        if mail.ctype.mimetype == "text/plain" {
            parts.push(
                mail.get_body()
                    .map_err(|_| anyhow::anyhow!("invalid_saved_plain_text"))?,
            );
        }
        for child in &mail.subparts {
            plain_parts(child, parts)?;
        }
        Ok(())
    }
    let mut texts = Vec::new();
    plain_parts(&actual_mail, &mut texts)?;
    ensure!(texts.len() == 1, "saved_plain_text_missing_or_ambiguous");
    let actual_signature = Signature::from_mime(actual)?;
    let normalize = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let expected_text = normalize(&plain_text(&expected_signature.html)?);
    let actual_html_text = normalize(&plain_text(&actual_signature.html)?);
    ensure!(
        expected_text == actual_html_text && actual_html_text == normalize(&texts[0]),
        "saved_html_text_mismatch"
    );
    Ok(())
}

fn collect(
    mail: &ParsedMail<'_>,
    html: &mut Option<String>,
    images: &mut BTreeMap<String, InlineImage>,
    depth: usize,
) -> Result<()> {
    ensure!(
        depth <= 12 && images.len() <= 50,
        "signature_template_too_complex"
    );
    ensure!(
        mail.get_content_disposition().disposition == DispositionType::Inline,
        "draft_has_attachments: only inline signature images are supported"
    );
    if matches!(mail.ctype.mimetype.as_str(), "text/plain" | "text/html") {
        ensure!(
            !mail.ctype.params.contains_key("name")
                && !mail
                    .get_content_disposition()
                    .params
                    .contains_key("filename"),
            "draft_has_attachments: named text parts are not supported"
        );
    }
    match mail.ctype.mimetype.as_str() {
        "multipart/alternative" | "multipart/related" | "multipart/mixed" => {
            ensure!(!mail.subparts.is_empty(), "invalid_template_mime");
            for part in &mail.subparts {
                collect(part, html, images, depth + 1)?;
            }
        }
        "text/html" => {
            ensure!(html.is_none(), "ambiguous_template_html");
            *html = Some(
                mail.get_body()
                    .map_err(|_| anyhow::anyhow!("invalid_signature_encoding"))?,
            );
        }
        "text/plain" => {}
        kind @ ("image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp") => {
            let cid = mail
                .headers
                .get_first_value("Content-ID")
                .ok_or_else(|| anyhow::anyhow!("signature_image_missing_content_id"))?;
            let cid = cid
                .strip_prefix('<')
                .and_then(|v| v.strip_suffix('>'))
                .unwrap_or(&cid)
                .to_owned();
            ensure!(
                !cid.is_empty()
                    && cid.len() <= 200
                    && cid
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"@._+-".contains(&b)),
                "invalid_signature_content_id"
            );
            let image = InlineImage {
                mime_type: kind.into(),
                bytes: mail
                    .get_body_raw()
                    .map_err(|_| anyhow::anyhow!("invalid_signature_image"))?,
            };
            ensure!(
                images.insert(cid, image).is_none(),
                "duplicate_signature_content_id"
            );
        }
        _ => bail!(
            "unsupported_template_part: use an HTML signature-only draft with inline raster images"
        ),
    }
    Ok(())
}

fn validate_signature(document: &Html, images: &BTreeMap<String, InlineImage>) -> Result<()> {
    for node in document.tree.root().descendants() {
        ensure!(
            node.ancestors().take(66).count() <= 64,
            "signature_html_too_deep"
        );
        ensure!(
            !matches!(
                node.value(),
                Node::Comment(_) | Node::ProcessingInstruction(_)
            ),
            "unsupported_signature_html: comments and processing instructions are not supported"
        );
        if let Node::Element(element) = node.value() {
            ensure!(
                [
                    "html",
                    "head",
                    "body",
                    "meta",
                    "title",
                    "p",
                    "div",
                    "span",
                    "br",
                    "h1",
                    "h2",
                    "h3",
                    "h4",
                    "h5",
                    "h6",
                    "ul",
                    "ol",
                    "li",
                    "blockquote",
                    "pre",
                    "code",
                    "strong",
                    "b",
                    "em",
                    "i",
                    "u",
                    "s",
                    "a",
                    "table",
                    "caption",
                    "thead",
                    "tbody",
                    "tfoot",
                    "tr",
                    "th",
                    "td",
                    "hr",
                    "img",
                    "font",
                    "center"
                ]
                .contains(&element.name()),
                "unsupported_signature_html: active elements and stylesheets are not supported"
            );
            for (name, value) in element.attrs() {
                ensure!(
                    !value.chars().any(char::is_control),
                    "invalid_signature_attribute"
                );
                match name {
                    "href" if element.name() == "a" => crate::compose::validate_link(value)?,
                    "src" if element.name() == "img" => {
                        if let Some(cid) = value.strip_prefix("cid:") {
                            ensure!(images.contains_key(cid), "signature_image_not_found");
                        } else {
                            let url = url::Url::parse(value)
                                .map_err(|_| anyhow::anyhow!("invalid_signature_image_url"))?;
                            ensure!(
                                url.scheme() == "https"
                                    && url.username().is_empty()
                                    && url.password().is_none(),
                                "invalid_signature_image_url"
                            );
                        }
                    }
                    "style" => validate_style(value)?,
                    "id" | "class" | "dir" | "lang" | "title" | "alt" | "align" | "valign"
                    | "width" | "height" | "border" | "cellpadding" | "cellspacing" | "colspan"
                    | "rowspan" | "scope" | "color" | "face" | "size" => {}
                    name if name.starts_with("data-") => {}
                    // Head metadata is discarded together with the source document's head.
                    "charset" | "http-equiv" | "content" | "name" if element.name() == "meta" => {}
                    _ => bail!("unsupported_signature_attribute"),
                }
            }
        }
    }
    Ok(())
}

fn validate_style(style: &str) -> Result<()> {
    for declaration in style.split(';').filter(|d| !d.trim().is_empty()) {
        let (property, value) = declaration
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("unsupported_signature_style"))?;
        let property = property.trim().to_ascii_lowercase();
        let value = value.trim().to_ascii_lowercase();
        ensure!(
            [
                "font",
                "font-family",
                "font-size",
                "font-weight",
                "font-style",
                "color",
                "background-color",
                "text-decoration",
                "text-align",
                "vertical-align",
                "line-height",
                "margin",
                "margin-top",
                "margin-bottom",
                "margin-left",
                "margin-right",
                "padding",
                "padding-top",
                "padding-bottom",
                "padding-left",
                "padding-right",
                "border",
                "border-top",
                "border-bottom",
                "border-left",
                "border-right",
                "border-collapse",
                "border-spacing",
                "width",
                "height",
                "max-width",
                "min-width",
                "display",
                "white-space"
            ]
            .contains(&property.as_str())
                && !["url", "expression", "var(", "attr("]
                    .iter()
                    .any(|s| value.contains(s))
                && value
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == ' ' || "#,.%()-'\"!".contains(c)),
            "unsupported_signature_style"
        );
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::compose::{EmailInput, prepare};

    pub(crate) const TEMPLATE: &[u8] = b"From: owner@example.test\r\nDate: Mon, 2 Nov 2026 10:00:00 +0000\r\nContent-Type: multipart/related; boundary=fixture\r\n\r\n--fixture\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html><body><div style=\"font-family:Arial;color:rgb(1, 2, 3)\">Signature<img src=\"cid:logo@example.test\" alt=\"Logo\"></div></body></html>\r\n--fixture\r\nContent-Type: image/png\r\nContent-ID: <logo@example.test>\r\nContent-Transfer-Encoding: base64\r\n\r\nc3ludGhldGljLWltYWdl\r\n--fixture--\r\n";

    #[test]
    fn builds_alternatives_from_final_html_and_preserves_signature_images() {
        let signature = Signature::from_mime(TEMPLATE).unwrap();
        let input = EmailInput {
            to: vec!["reader@example.test".into()],
            cc: vec![],
            bcc: vec![],
            subject: "Novità ".repeat(30),
            html: "<p>Caffè &amp; attività</p>".into(),
        };
        let bytes = signature.mime(&prepare(&input).unwrap()).unwrap();
        let mail = mailparse::parse_mail(&bytes).unwrap();
        assert_eq!(
            mail.headers.get_first_value("Subject").unwrap(),
            input.subject
        );
        assert!(bytes.split(|b| *b == b'\n').all(|line| line.len() <= 998));
        assert_eq!(mail.ctype.mimetype, "multipart/related");
        assert!(mail.ctype.params["boundary"].len() <= 70);
        let alternatives = &mail.subparts[0];
        assert_eq!(alternatives.ctype.mimetype, "multipart/alternative");
        assert!(alternatives.ctype.params["boundary"].len() <= 70);
        let text = alternatives.subparts[0].get_body().unwrap();
        let html = alternatives.subparts[1].get_body().unwrap();
        assert_eq!(text, plain_text(&html).unwrap());
        assert!(text.contains("Signature"));
        assert!(html.contains("font-family:Arial"));
        assert_eq!(html.matches("Signature").count(), 1);
        assert_eq!(mail.subparts[1].get_body_raw().unwrap(), b"synthetic-image");
        signature.check_existing_images(&bytes).unwrap();
        verify_saved(&bytes, &bytes).unwrap();
        let mut encoded_text = String::new();
        encode_body(&mut encoded_text, text.as_bytes());
        let altered = String::from_utf8(bytes.clone())
            .unwrap()
            .replace(&encoded_text, "RGlmZmVyZW50IHRleHQ=\r\n");
        assert!(verify_saved(&bytes, altered.as_bytes()).is_err());
        let bad_text = String::from_utf8(bytes.clone()).unwrap().replacen(
            "Content-Type: text/plain",
            "Content-Type: text/other",
            1,
        );
        assert!(verify_saved(&bytes, bad_text.as_bytes()).is_err());
        let changed = String::from_utf8(TEMPLATE.to_vec())
            .unwrap()
            .replace("c3ludGhldGljLWltYWdl", "Y2hhbmdlZA==");
        assert!(signature.check_existing_images(changed.as_bytes()).is_err());

        let inherited = String::from_utf8(TEMPLATE.to_vec())
            .unwrap()
            .replace("<html>", "<html dir=\"ltr\">")
            .replace("<body>", "<body style=\"font-size:11pt\">");
        let signature = Signature::from_mime(inherited.as_bytes()).unwrap();
        let bytes = signature.mime(&prepare(&input).unwrap()).unwrap();
        let mail = mailparse::parse_mail(&bytes).unwrap();
        let html = mail.subparts[0].subparts[1].get_body().unwrap();
        assert!(html.contains("font-size:11pt"));
        assert!(html.contains("dir=\"ltr\""));
    }

    #[test]
    fn rejects_attachments_active_signatures_missing_images_and_unsafe_css() {
        let template = String::from_utf8(TEMPLATE.to_vec()).unwrap();
        for bad in [
            template.replace("image/png", "application/pdf"),
            template.replace("<div style=", "<div onclick=\"secret\" style="),
            template.replace("cid:logo@example.test", "cid:missing"),
            template.replace(
                "color:rgb(1, 2, 3)",
                "background:url(https://example.test/secret)",
            ),
            template.replace("<html>", "<html><script>secret</script>"),
            template.replace("<html>", "<html><!--secret-->"),
            template.replace(
                "text/html; charset=utf-8",
                "text/html; charset=utf-8; name=secret.html",
            ),
        ] {
            let error = Signature::from_mime(bad.as_bytes())
                .err()
                .expect("unsafe template");
            assert!(!error.to_string().contains("secret"));
        }
    }
}
