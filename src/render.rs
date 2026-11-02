//! Deterministic presentation of validated authored HTML. Never fetches resources.
use crate::compose::EmailTheme;
use anyhow::{Result, ensure};
use scraper::{Html, Node};

fn accent(theme: &EmailTheme) -> Result<String> {
    let value = &theme.accent;
    ensure!(
        value.len() == 7
            && value.starts_with('#')
            && value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit),
        "invalid_theme_accent: use #RRGGBB"
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
        1.05 / (luminance + 0.05) >= 4.5,
        "invalid_theme_accent: contrast against white must be at least 4.5:1"
    );
    Ok(value.to_ascii_lowercase())
}

pub(crate) fn render(mut fragment: Html, theme: &EmailTheme) -> Result<String> {
    let accent = accent(theme)?;
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
        let style = match element.name() {
            "p" => "margin:0 0 16px".into(),
            "h1" => format!("margin:24px 0 12px;font-size:26px;line-height:1.3;color:{accent}"),
            "h2" => format!("margin:24px 0 12px;font-size:22px;line-height:1.3;color:{accent}"),
            "h3" | "h4" | "h5" | "h6" => format!("margin:20px 0 10px;font-size:18px;line-height:1.4;color:{accent}"),
            "a" => format!("color:{accent};text-decoration:underline"),
            "ul" | "ol" => "margin:0 0 16px;padding-left:24px".into(),
            "li" => "margin-bottom:6px".into(),
            "blockquote" => "margin:16px 0;padding:8px 16px;border-left:3px solid #667085".into(),
            "pre" => "margin:16px 0;white-space:pre-wrap;font-family:Consolas,monospace".into(),
            "code" => "font-family:Consolas,monospace".into(),
            "table" => "width:100%;border-collapse:collapse;margin:16px 0".into(),
            "th" => "padding:8px;border:1px solid #667085;text-align:left;vertical-align:top;font-weight:bold".into(),
            "td" => "padding:8px;border:1px solid #667085;vertical-align:top".into(),
            "caption" => "text-align:left;font-weight:bold;margin-bottom:8px".into(),
            "hr" => "border:0;border-top:1px solid #667085;margin:24px 0".into(),
            _ => continue,
        };
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
    Ok(format!(
        "<div style=\"width:100%;max-width:640px;box-sizing:border-box;font-family:Arial,Helvetica,sans-serif;font-size:16px;line-height:1.5;color:#202124;background-color:#ffffff;overflow-wrap:anywhere\">{}</div>",
        fragment.root_element().inner_html()
    ))
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
        email.theme.accent = "#123456".into();
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
            email.theme.accent = color.into();
            let error = prepare(&email).err().unwrap().to_string();
            assert!(error.starts_with("invalid_theme_accent:"));
            assert!(!error.contains(color));
        }
        assert!(
            accent(&EmailTheme {
                accent: "#767676".into()
            })
            .is_ok()
        );
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
}
