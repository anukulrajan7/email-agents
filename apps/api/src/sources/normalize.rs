//! Provider-independent normalization helpers.

/// `"Jane Doe <Jane@Acme.com>"` → `"jane@acme.com"`.
pub fn address(raw: &str) -> String {
    let raw = raw.trim();
    let inner = match (raw.rfind('<'), raw.rfind('>')) {
        (Some(start), Some(end)) if start < end => &raw[start + 1..end],
        _ => raw,
    };
    inner.trim().trim_matches('"').to_lowercase()
}

/// Email HTML made safe to display: no scripts, forms, frames, event handlers or
/// `javascript:` URLs; links open in a new tab without referrer; only http(s)/mailto
/// links and https images survive. The UI still renders it in a sandboxed iframe.
pub fn safe_email_html(html: &str) -> String {
    use std::collections::HashSet;
    let mut builder = ammonia::Builder::default();
    builder
        .add_generic_attributes(["style", "align", "valign", "width", "height", "bgcolor", "border", "cellpadding", "cellspacing"])
        .add_tags(["center", "font"])
        .add_tag_attributes("font", ["color", "face", "size"])
        .url_schemes(HashSet::from(["http", "https", "mailto"]))
        .link_rel(Some("noopener noreferrer nofollow"))
        .set_tag_attribute_value("a", "target", "_blank");
    let cleaned = builder.clean(html).to_string();
    // Plain-http images leak the reader's IP over cleartext and break mixed content: upgrade them.
    cleaned.replace("src=\"http://", "src=\"https://")
}

/// `"Jane Doe" <jane@acme.com>` → `Some("Jane Doe")`; bare addresses → `None`.
pub fn display_name(raw: &str) -> Option<String> {
    let name = raw.split_once('<')?.0.trim().trim_matches('"').trim();
    (!name.is_empty() && !name.contains('@')).then(|| truncate_chars(name, 120))
}

pub fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((idx, _)) => s[..idx].to_string(),
        None => s.to_string(),
    }
}

/// Crude tag stripper used only when a message has no text part. Output is
/// displayed as plain text, never re-rendered as HTML.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut tag = String::new();
    let mut skip_depth = 0usize;
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                let name: String = tag.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                let name = name.to_ascii_lowercase();
                let closing = tag.starts_with('/');
                if name == "script" || name == "style" {
                    if closing { skip_depth = skip_depth.saturating_sub(1) } else { skip_depth += 1 }
                }
                if matches!(name.as_str(), "br" | "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3") {
                    out.push('\n');
                }
            }
            _ if in_tag => tag.push(c),
            _ if skip_depth > 0 => {}
            _ => out.push(c),
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    let mut lines: Vec<&str> = decoded.lines().map(str::trim).collect();
    lines.dedup_by(|a, b| a.is_empty() && b.is_empty());
    lines.join("\n").trim().to_string()
}

/// Extracts `From:` from a Gmail/Outlook/Apple style forwarded block. The result is
/// *parsed, not verified*, and must be labelled as such in the UI.
pub fn forwarded_original_sender(body: &str) -> Option<String> {
    let lines: Vec<&str> = body.lines().map(str::trim).collect();
    let marker = lines.iter().position(|l| {
        let lower = l.to_lowercase();
        lower.contains("forwarded message") || lower.starts_with("begin forwarded message")
    })?;
    lines
        .iter()
        .skip(marker + 1)
        .take(8)
        .find_map(|l| l.strip_prefix("From:").or_else(|| l.strip_prefix("from:")))
        .map(address)
        .filter(|a| a.contains('@') && !a.contains(char::is_whitespace))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitises_email_html() {
        let dirty = r#"<p onclick="x()">Hi <a href="javascript:alert(1)">bad</a> <a href="https://acme.com">ok</a></p>
<script>alert(1)</script><img src="http://img.example/a.png"><img src="cid:logo"><form><input></form><iframe src="https://x"></iframe>"#;
        let clean = safe_email_html(dirty);
        assert!(!clean.contains("script") && !clean.contains("onclick") && !clean.contains("javascript:"));
        assert!(!clean.contains("<form") && !clean.contains("<iframe") && !clean.contains("cid:"));
        assert!(clean.contains(r#"href="https://acme.com""#) && clean.contains(r#"target="_blank""#));
        assert!(clean.contains("noopener"));
        assert!(clean.contains(r#"src="https://img.example/a.png""#));
    }

    #[test]
    fn parses_display_names() {
        assert_eq!(display_name("\"Jane Doe\" <jane@acme.com>").as_deref(), Some("Jane Doe"));
        assert_eq!(display_name("jane@acme.com"), None);
        assert_eq!(display_name("<jane@acme.com>"), None);
    }

    #[test]
    fn parses_addresses() {
        assert_eq!(address("Jane Doe <Jane@Acme.com>"), "jane@acme.com");
        assert_eq!(address(" bob@x.io "), "bob@x.io");
    }

    #[test]
    fn finds_forwarded_sender() {
        let body = "FYI\n\n---------- Forwarded message ---------\nFrom: Priya <priya@acme.com>\nDate: Mon\nSubject: Timesheet";
        assert_eq!(forwarded_original_sender(body).as_deref(), Some("priya@acme.com"));
        assert_eq!(forwarded_original_sender("From: someone@x.com"), None);
    }

    #[test]
    fn strips_html_and_scripts() {
        let text = html_to_text("<p>Hello&nbsp;<b>there</b></p><script>alert(1)</script><div>Bye &amp; thanks</div>");
        assert_eq!(text, "Hello there\n\nBye & thanks");
    }

    #[test]
    fn truncates_on_char_boundary() {
        assert_eq!(truncate_chars("héllo", 2), "hé");
    }
}
