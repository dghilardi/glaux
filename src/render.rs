//! Deterministic presentation of validated authored HTML. Never fetches resources.
use crate::compose::EmailTheme;
use anyhow::{Result, ensure};
use scraper::{Html, Node};
use std::sync::OnceLock;

pub(crate) const SIGNATURE_SLOT: &str = "<!--glaux-signature-slot-->";

fn document(content: &str, text: &str) -> Result<String> {
    // Only validated HTML enters mj-text. Includes, remote loaders and arbitrary MJML
    // are not exposed, and MRML is compiled without filesystem or HTTP loaders.
    let source = format!(
        r##"<mjml><mj-body width="640px" background-color="#ffffff"><mj-section padding="0 16px" text-align="left"><mj-column width="100%" padding="0"><mj-text font-family="Arial,Helvetica,sans-serif" font-size="16px" line-height="1.5" color="{text}" padding="0" align="left">{content}</mj-text><mj-raw>{SIGNATURE_SLOT}</mj-raw></mj-column></mj-section></mj-body></mjml>"##
    );
    let root = mrml::parse(&source).map_err(|_| anyhow::anyhow!("email_render_failed"))?;
    root.element
        .render(&mrml::prelude::render::RenderOptions::default())
        .map_err(|_| anyhow::anyhow!("email_render_failed"))
}

fn normalized(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

// Derive the allowlist from the same fixed template/version, never from message input.
// It permits only the engine's own head CSS and Outlook compatibility comments.
fn allowances() -> Option<&'static (Vec<String>, Vec<String>)> {
    static ALLOWED: OnceLock<Option<(Vec<String>, Vec<String>)>> = OnceLock::new();
    ALLOWED
        .get_or_init(|| {
            let html = Html::parse_document(&document("", "#202124").ok()?);
            let comments = html
                .tree
                .root()
                .descendants()
                .filter_map(|n| {
                    if let Node::Comment(value) = n.value() {
                        Some(normalized(value))
                    } else {
                        None
                    }
                })
                .collect();
            let styles = html
                .select(&scraper::Selector::parse("head style").unwrap())
                .map(|e| normalized(&e.text().collect::<String>()))
                .collect();
            Some((comments, styles))
        })
        .as_ref()
}

pub(crate) fn allows_comment(value: &str) -> bool {
    allowances().is_some_and(|a| a.0.contains(&normalized(value)))
}

pub(crate) fn allows_head_style(value: &str) -> bool {
    allowances().is_some_and(|a| a.1.contains(&normalized(value)))
}

fn color(value: &str, name: &str, text: bool) -> Result<String> {
    ensure!(
        value.len() == 7
            && value.starts_with('#')
            && value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit),
        "invalid_theme_{name}: use #RRGGBB"
    );
    let mut luminance = 0.0;
    for (offset, weight) in [(1, 0.2126), (3, 0.7152), (5, 0.0722)] {
        let channel = u8::from_str_radix(&value[offset..offset + 2], 16).unwrap() as f64 / 255.0;
        let linear = if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        };
        luminance += weight * linear;
    }
    ensure!(
        !text || 1.05 / (luminance + 0.05) >= 4.5,
        "invalid_theme_{name}: contrast against white must be at least 4.5:1"
    );
    Ok(value.to_ascii_lowercase())
}

pub(crate) fn render(mut fragment: Html, theme: &EmailTheme) -> Result<String> {
    let theme = theme.resolve(&EmailTheme::default());
    let accent = color(theme.accent.as_deref().unwrap(), "accent", true)?;
    let text = color(theme.text.as_deref().unwrap(), "text", true)?;
    let link = color(theme.link.as_deref().unwrap(), "link", true)?;
    let border = color(theme.border.as_deref().unwrap(), "border", false)?;
    let decoration = theme
        .decoration
        .as_deref()
        .filter(|v| *v != "none")
        .map(|v| color(v, "decoration", false))
        .transpose()?;
    let mut decorated = false;
    let ids = fragment
        .tree
        .root()
        .descendants()
        .map(|n| n.id())
        .collect::<Vec<_>>();
    for id in ids {
        let mut node = fragment.tree.get_mut(id).unwrap();
        let Node::Element(element) = node.value() else {
            continue;
        };
        let mut style = match element.name() {
            "p" => "margin:0 0 16px".into(),
            "h1" => format!("margin:24px 0 12px;font-size:26px;line-height:1.3;color:{accent}"),
            "h2" => format!("margin:24px 0 12px;font-size:22px;line-height:1.3;color:{accent}"),
            "h3" | "h4" | "h5" | "h6" => format!("margin:20px 0 10px;font-size:18px;line-height:1.4;color:{accent}"),
            "a" => format!("color:{link};text-decoration:underline"),
            "ul" | "ol" => "margin:0 0 16px 24px;padding:0".into(),
            "li" => "margin-bottom:6px".into(),
            "blockquote" => format!("margin:16px 0;padding:8px 16px;border-left:3px solid {border}"),
            "pre" => "margin:16px 0;white-space:pre-wrap;font-family:Consolas,monospace".into(),
            "code" => "font-family:Consolas,monospace".into(),
            "table" => "width:100%;border-collapse:collapse;margin:16px 0".into(),
            "th" => format!("font-family:Arial,Helvetica,sans-serif;font-size:16px;line-height:1.5;padding:8px;border:1px solid {border};color:{text};text-align:left;vertical-align:top;font-weight:bold"),
            "td" => format!("font-family:Arial,Helvetica,sans-serif;font-size:16px;line-height:1.5;padding:8px;border:1px solid {border};color:{text};vertical-align:top"),
            "caption" => "font-family:Arial,Helvetica,sans-serif;font-size:16px;text-align:left;font-weight:bold;margin-bottom:8px".into(),
            "hr" => format!("border:0;border-top:1px solid {border};margin:24px 0"),
            _ => continue,
        };
        if !decorated && matches!(element.name(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
            decorated = true;
            if let Some(color) = &decoration {
                style.push_str(&format!(
                    ";border-bottom:2px solid {color};padding-bottom:12px"
                ));
            }
        }
        // All authored attributes have already passed compose::prepare. Only this renderer
        // can introduce styles, and the accent above contains no CSS syntax.
        let mut name = element.name.clone();
        name.local = "style".into();
        name.ns = "".into();
        name.prefix = None;
        let index = element
            .attrs
            .binary_search_by(|(key, _)| key.cmp(&name))
            .unwrap_err();
        element.attrs.insert(index, (name, style.into()));
    }
    let html = fragment.root_element().inner_html();
    // Word-based clients do not consistently honor white-space on pre. Explicit
    // breaks preserve the authored newlines without interpreting text as markup.
    let mut content = String::new();
    let mut rest = html.as_str();
    while let Some(start) = rest.find("<pre ") {
        let end = rest[start..].find("</pre>").unwrap() + start + "</pre>".len();
        content.push_str(&rest[..start]);
        content.push_str(
            &rest[start..end]
                .replace("\r\n", "\n")
                .replace('\r', "\n")
                .replace('\n', "<br>"),
        );
        rest = &rest[end..];
    }
    content.push_str(rest);
    document(
        &format!("<div style=\"overflow-wrap:anywhere;color:{text}\">{content}</div>"),
        &text,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::{prepare, tests::input};

    #[test]
    fn bounds_expansion_before_mailbox_access() {
        let email = input(&"<h1>Title</h1>".repeat(4000));
        assert_eq!(
            prepare(&email).err().unwrap().to_string(),
            "rendered_html_too_large"
        );
    }

    #[test]
    fn renders_only_after_validation_and_preserves_semantics() {
        let mut email = input(
            "<h2>Update</h2><p><a href=\"https://example.test/?a=1&amp;b=2\">Report</a></p><table><tr><th scope=\"col\">Item</th><th>State</th></tr><tr><td colspan=\"2\">Ready</td></tr></table><pre>code &lt;value&gt;</pre>",
        );
        email.theme.accent = Some("#123456".into());
        let rendered = prepare(&email).unwrap();
        assert!(rendered.html.contains("color:#123456"));
        assert!(rendered.html.contains("max-width:640px"));
        assert!(rendered.html.contains("colspan=\"2\""));
        assert!(rendered.html.contains("scope=\"col\""));
        for text in [
            "Update",
            "Ready",
            "https://example.test/?a=1&b=2",
            "code <value>",
        ] {
            assert!(rendered.text.contains(text), "{text}");
        }
        assert_eq!(rendered.html, prepare(&email).unwrap().html);
        email.html = "<p style=\"color:red\">No</p>".into();
        assert!(prepare(&email).is_err());
    }

    #[test]
    fn rejects_unsafe_or_low_contrast_colors_without_echoing_values() {
        for color in [
            "#ffffff",
            "#aaaaaa",
            "#777777",
            "red",
            "#123456;display:none",
            "#éabcd",
            "#12345",
        ] {
            let mut email = input("<p>Hello</p>");
            email.theme.accent = Some(color.into());
            let error = prepare(&email).err().unwrap().to_string();
            assert!(error.starts_with("invalid_theme_accent:"));
            assert!(!error.contains(color));
        }
        assert!(color("#767676", "accent", true).is_ok());
    }

    #[test]
    fn validates_each_color_and_only_decorates_the_first_heading() {
        let mut email = input(
            "<h1>Title</h1><h2>Section</h2><p>Body</p><a href=\"https://example.test\">Link</a><table><tr><td>Cell</td></tr></table><hr>",
        );
        email.theme = serde_json::from_value(serde_json::json!({
            "text":"#292929", "accent":"#123456", "link":"#342156",
            "border":"#aabbcc", "decoration":"#bbccdd"
        }))
        .unwrap();
        let rendered = prepare(&email).unwrap();
        assert_eq!(
            rendered
                .html
                .matches("border-bottom:2px solid #bbccdd")
                .count(),
            1
        );
        assert!(
            rendered
                .html
                .contains("border:1px solid #aabbcc;color:#292929")
        );
        assert!(
            rendered
                .html
                .contains("color:#342156;text-decoration:underline")
        );
        for field in ["accent", "text", "link", "border", "decoration"] {
            for value in ["#123456;display:none", "#éabcd"] {
                let mut theme = serde_json::to_value(&email.theme).unwrap();
                theme[field] = value.into();
                let mut invalid = email.clone();
                invalid.theme = serde_json::from_value(theme).unwrap();
                let error = prepare(&invalid).err().unwrap().to_string();
                assert_eq!(error, format!("invalid_theme_{field}: use #RRGGBB"));
            }
        }
        for field in ["accent", "text", "link"] {
            let mut theme = serde_json::to_value(&email.theme).unwrap();
            theme[field] = "#ffffff".into();
            let mut invalid = email.clone();
            invalid.theme = serde_json::from_value(theme).unwrap();
            assert!(prepare(&invalid).is_err());
        }
    }

    #[test]
    fn theme_is_optional_and_rejects_unknown_options() {
        let email: crate::compose::EmailInput =
            serde_json::from_str(r#"{"subject":"Synthetic","html":"<p>Body</p>"}"#).unwrap();
        assert_eq!(email.theme.accent, EmailTheme::default().accent);
        assert!(
            serde_json::from_str::<crate::compose::EmailInput>(
                r#"{"subject":"Synthetic","html":"<p>Body</p>","theme":{"css":"display:none"}}"#,
            )
            .is_err()
        );
    }

    #[test]
    fn mjml_layout_keeps_outlook_width_and_explicit_code_breaks() {
        let prepared = prepare(&input(
            "<ul><li>One</li><li>Two</li></ul><pre>first\nsecond &lt;tag&gt;</pre>",
        ))
        .unwrap();
        assert!(prepared.html.starts_with("<!doctype html>"));
        assert!(prepared.html.contains("<!--[if mso | IE]>"));
        assert!(prepared.html.contains("width=\"640\""));
        assert!(prepared.html.contains("margin:0 0 16px 24px"));
        assert!(prepared.html.contains("first<br />second &lt;tag&gt;"));
        assert!(prepared.text.contains("first\nsecond <tag>"));
        assert!(!prepared.text.contains('─'));
        assert!(!prepared.html.contains("fonts.googleapis"));
    }
}
