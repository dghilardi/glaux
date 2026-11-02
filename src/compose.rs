//! Local email composition. No browser, external resources or independently supplied text body.
use anyhow::{Result, bail, ensure};
use rmcp::schemars;
use scraper::{Html, Node};
use serde::{Deserialize, Serialize};

pub const MAX_HTML_BYTES: usize = 256 * 1024;
pub const MAX_RECIPIENTS: usize = 100;

#[derive(Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmailInput {
    /// Bare email addresses, without display names. At most 100 recipients in total.
    #[serde(default)]
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Vec<String>,
    #[serde(default)]
    pub bcc: Vec<String>,
    /// Single-line subject, at most 255 characters and 998 UTF-8 bytes.
    pub subject: String,
    /// Semantic HTML fragment (p, headings, lists, links, tables, emphasis). No CSS or images.
    /// Plain text is derived automatically. Maximum 256 KiB.
    pub html: String,
}

#[derive(Clone, Serialize)]
pub struct PreparedEmail {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub html: String,
    pub text: String,
}

pub fn prepare(input: &EmailInput) -> Result<PreparedEmail> {
    ensure!(
        input.to.len() + input.cc.len() + input.bcc.len() <= MAX_RECIPIENTS,
        "too_many_recipients: at most 100 recipients are supported"
    );
    for address in input.to.iter().chain(&input.cc).chain(&input.bcc) {
        ensure!(
            address.len() <= 254
                && address.is_ascii()
                && !address.chars().any(char::is_whitespace)
                && email_address::EmailAddress::is_valid(address),
            "invalid_recipient: use a bare email address"
        );
    }
    ensure!(
        input.subject.len() <= 998
            && input.subject.chars().count() <= 255
            && !input.subject.chars().any(unsafe_control),
        "invalid_subject: use a single line of at most 255 characters and 998 bytes"
    );
    ensure!(
        !input.html.trim().is_empty() && input.html.len() <= MAX_HTML_BYTES,
        "invalid_html_size: provide a nonempty HTML fragment of at most 256 KiB"
    );
    ensure!(
        !input
            .html
            .chars()
            .any(|c| unsafe_control(c) && c != '\n' && c != '\t' && c != '\r'),
        "invalid_html: control characters are not supported"
    );
    let fragment = Html::parse_fragment(&input.html);
    ensure!(
        fragment.errors.is_empty(),
        "invalid_html: malformed HTML fragment"
    );
    let mut elements = 0;
    for node in fragment.tree.root().descendants() {
        ensure!(
            node.ancestors().take(66).count() <= 64,
            "invalid_html: nesting exceeds 64 levels"
        );
        match node.value() {
            Node::Element(element) => {
                let tag = element.name();
                if tag == "html" {
                    continue;
                }
                elements += 1;
                ensure!(elements <= 10_000, "invalid_html: too many elements");
                ensure!(
                    [
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
                        "hr"
                    ]
                    .contains(&tag),
                    "unsupported_html_element: use semantic HTML without active content, styles or images"
                );
                for (name, value) in element.attrs() {
                    match (tag, name) {
                        ("a", "href") => validate_link(value)?,
                        ("a", "title") | ("th", "scope") => {
                            ensure!(!value.chars().any(unsafe_control), "invalid_html_attribute");
                        }
                        ("td" | "th", "colspan" | "rowspan") => {
                            ensure!(
                                value.parse::<u16>().is_ok_and(|n| (1..=100).contains(&n)),
                                "invalid_table_span"
                            );
                        }
                        _ => bail!(
                            "unsupported_html_attribute: styles, event handlers and arbitrary attributes are not supported"
                        ),
                    }
                }
            }
            Node::Text(text) => ensure!(
                !text
                    .chars()
                    .any(|c| unsafe_control(c) && c != '\n' && c != '\r' && c != '\t'),
                "invalid_html: encoded control characters are not supported"
            ),
            Node::Comment(_) | Node::Doctype(_) | Node::ProcessingInstruction(_) => {
                bail!("invalid_html: comments and document declarations are not supported");
            }
            _ => {}
        }
    }
    let html = fragment.root_element().inner_html();
    let text = plain_text(&html)?;
    ensure!(
        !text.trim().is_empty(),
        "empty_body: provide visible message text"
    );
    Ok(PreparedEmail {
        to: input.to.clone(),
        cc: input.cc.clone(),
        bcc: input.bcc.clone(),
        subject: input.subject.clone(),
        html,
        text,
    })
}

pub(crate) fn plain_text(html: &str) -> Result<String> {
    html2text::from_read(html.as_bytes(), 100)
        .map_err(|_| anyhow::anyhow!("plain_text_conversion_failed"))
}

fn unsafe_control(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

pub(crate) fn validate_link(value: &str) -> Result<()> {
    ensure!(
        !value.chars().any(|c| c.is_control() || c.is_whitespace()),
        "invalid_link"
    );
    let url = url::Url::parse(value).map_err(|_| anyhow::anyhow!("invalid_link"))?;
    ensure!(
        matches!(url.scheme(), "https" | "http" | "mailto")
            && url.username().is_empty()
            && url.password().is_none(),
        "invalid_link: only HTTP, HTTPS and mailto links without credentials are supported"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn input(html: &str) -> EmailInput {
        EmailInput {
            to: vec!["reader@example.test".into()],
            cc: vec![],
            bcc: vec![],
            subject: "Synthetic subject".into(),
            html: html.into(),
        }
    }

    #[test]
    fn one_source_preserves_unicode_lists_links_and_tables() {
        let email = prepare(&input("<h2>Novità &amp; attività</h2><p>Caffè <strong>oggi</strong>.</p><ul><li>Primo</li><li>Secondo</li></ul><p><a href=\"https://example.test/report\">Report</a></p><table><tr><th>Voce</th><th>Valore</th></tr><tr><td>Totale</td><td>42</td></tr></table>")).unwrap();
        for value in [
            "Novità & attività",
            "Caffè",
            "Primo",
            "Secondo",
            "https://example.test/report",
            "Totale",
            "42",
        ] {
            assert!(email.text.contains(value), "{value}");
        }
        assert_eq!(email.text, plain_text(&email.html).unwrap());
    }

    #[test]
    fn rejects_active_ambiguous_and_oversized_inputs_without_echoing_them() {
        for html in [
            "<script>secret</script>",
            "<p onclick=\"secret\">Hello</p>",
            "<a href=\"javascript:secret\">Link</a>",
            "<a href=\"https://user:secret@example.test\">Link</a>",
            "<img src=\"https://example.test/secret\">",
            "<p style=\"display:none\">secret</p>",
            "<div><p>x</div></p>",
            "<p>&#27;secret</p>",
            "<p> </p>",
            "<!--secret--><p>x</p>",
        ] {
            let error = prepare(&input(html)).err().expect("invalid input");
            assert!(!error.to_string().contains("secret"));
        }
        let mut email = input("<p>Body</p>");
        email.subject = "secret\r\nBcc: hidden@example.test".into();
        assert!(prepare(&email).is_err());
        email.subject = "Synthetic".into();
        email.to = vec!["secret\n@example.test".into()];
        assert!(prepare(&email).is_err());
        email.to = vec!["robot@example.test".into(); MAX_RECIPIENTS + 1];
        assert!(prepare(&email).is_err());
        email.to.clear();
        email.html = format!("{}Text{}", "<div>".repeat(70), "</div>".repeat(70));
        assert!(prepare(&email).is_err());
        email.html = "<p>Body</p>".into();
        email.subject = "x".repeat(256);
        assert!(prepare(&email).is_err());
        email.subject = "Synthetic".into();
        email.html = "x".repeat(MAX_HTML_BYTES + 1);
        assert!(prepare(&email).is_err());
    }
}
