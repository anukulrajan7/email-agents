//! Cleans plain-text email bodies for display and for model input: no HTML comments (incl.
//! Outlook conditional comments), no stray tags, `<https://…>` wrappers become bare URLs next to
//! their anchor text, no runs of blank lines, no immediately repeated blocks. When the text part
//! is empty or mostly junk, the text is derived from the HTML part instead.

use std::sync::LazyLock;

use regex::Regex;

use crate::sources::normalize;

const TAGS: &str = "html|head|body|title|meta|link|style|script|div|span|p|br|hr|table|thead|tbody|tfoot|tr|td|th|ul|ol|li|a|b|i|u|s|em|strong|small|big|sup|sub|font|center|img|h[1-6]|blockquote|pre|code|section|header|footer|nav|article|button|form|input|label|o:p|v:[a-z]+|xml|!doctype";

static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").expect("valid regex"));
static COND_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<!\[(?:end)?if[^\]]*\]>?(?:--!?>)?").expect("valid regex"));
static BLOCKS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<(head|style|script|title)\b[^>]*>.*?</(?:head|style|script|title)\s*>").expect("valid regex"));
static LINE_BREAK_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)</?(?:br|p|div|tr|li|h[1-6]|table|ul|ol|blockquote|hr)\b[^>\n]*>").expect("valid regex"));
static STRAY_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"(?i)</?(?:{TAGS})(?:[\s/][^>\n]*)?>")).expect("valid regex"));
static ANCHOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<a\s[^>]*?href\s*=\s*["']([^"']+)["'][^>]*>(.*?)</a\s*>"#).expect("valid regex"));
/// `Anchor text\n<https://…>`: the URL belongs to the line above.
static URL_ON_NEXT_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^([^\n<]*\S)[ \t]*\n[ \t]*<(https?://[^\s<>]+)>[ \t]*$").expect("valid regex"));
/// `<https://…>Anchor text`: the URL precedes its text on the same line.
static URL_BEFORE_TEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<(https?://[^\s<>]+)>([\p{L}\p{N}][^<\n]*)").expect("valid regex"));
static URL_WRAPPER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<(https?://[^\s<>]+)>").expect("valid regex"));
static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https?://\S+").expect("valid regex"));
static IMAGE_PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[(?:image|cid):[^\]]*\]").expect("valid regex"));
static BLANK_RUNS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").expect("valid regex"));

/// Below this many letters/digits (outside URLs and image placeholders) a text part is junk.
const MIN_MEANINGFUL_CHARS: usize = 20;
/// Repeated blocks shorter than this (e.g. "Thanks") are left alone.
const MIN_DUPLICATE_CHARS: usize = 20;

fn invisible(c: char) -> bool {
    matches!(c, '\u{200b}'..='\u{200d}' | '\u{2060}' | '\u{feff}' | '\u{00ad}' | '\u{034f}')
}

fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&zwnj;", "")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&rsquo;", "\u{2019}")
        .replace("&amp;", "&")
}

fn strip_markup(text: &str) -> String {
    let text = COMMENT.replace_all(text, "");
    let text = COND_END.replace_all(&text, "");
    let text = BLOCKS.replace_all(&text, "");
    let text = LINE_BREAK_TAG.replace_all(&text, "\n");
    let stripped = STRAY_TAG.replace_all(&text, "");
    if stripped == text { stripped.into_owned() } else { decode_entities(&stripped) }
}

fn link_urls(text: &str) -> String {
    let text = URL_ON_NEXT_LINE.replace_all(text, "$1 <$2>");
    let text = URL_BEFORE_TEXT.replace_all(&text, |c: &regex::Captures| format!("{} {}", c[2].trim_end(), &c[1]));
    URL_WRAPPER.replace_all(&text, "$1").into_owned()
}

fn normalize_paragraph(p: &str) -> String {
    p.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Removes a block of paragraphs that is immediately repeated (`A B A B` → `A B`).
fn drop_repeated_blocks(text: &str) -> String {
    let mut paragraphs: Vec<&str> = text.split("\n\n").collect();
    let keys = |ps: &[&str]| ps.iter().map(|p| normalize_paragraph(p)).collect::<Vec<_>>();
    let mut changed = true;
    while changed {
        changed = false;
        let normalized = keys(&paragraphs);
        'search: for size in (1..=paragraphs.len() / 2).rev() {
            for start in 0..=paragraphs.len() - 2 * size {
                let (first, second) = (&normalized[start..start + size], &normalized[start + size..start + 2 * size]);
                let len: usize = first.iter().map(String::len).sum();
                if first == second && len >= MIN_DUPLICATE_CHARS {
                    paragraphs.drain(start + size..start + 2 * size);
                    changed = true;
                    break 'search;
                }
            }
        }
    }
    paragraphs.join("\n\n")
}

/// Cleans a plain-text body. Idempotent.
pub fn clean_text(raw: &str) -> String {
    let text = raw.replace("\r\n", "\n").replace('\r', "\n").replace('\u{a0}', " ");
    let text: String = text.chars().filter(|c| !invisible(*c)).collect();
    let text = strip_markup(&text);
    let text = link_urls(&text);
    let text = text.lines().map(str::trim_end).collect::<Vec<_>>().join("\n");
    let text = BLANK_RUNS.replace_all(text.trim(), "\n\n").into_owned();
    drop_repeated_blocks(&text)
}

/// True when the text carries (almost) no readable words once URLs and image placeholders are ignored.
pub fn is_junk(text: &str) -> bool {
    let without_urls = URL.replace_all(text, "");
    let readable = IMAGE_PLACEHOLDER.replace_all(&without_urls, "");
    readable.chars().filter(|c| c.is_alphanumeric()).count() < MIN_MEANINGFUL_CHARS
}

/// Readable text from an HTML part: comments and head/style gone, links kept as `text url`.
pub fn html_to_clean_text(html: &str) -> String {
    let html = COMMENT.replace_all(html, "");
    let html = COND_END.replace_all(&html, "");
    let html = ANCHOR.replace_all(&html, |c: &regex::Captures| {
        let inner = normalize::html_to_text(&c[2]);
        let href = c[1].trim();
        if inner.is_empty() || !href.starts_with("http") || inner.contains(href) {
            inner
        } else {
            format!("{inner} {href}")
        }
    });
    clean_text(&normalize::html_to_text(&BLOCKS.replace_all(&html, "")))
}

/// The text to store: the cleaned text part, or text derived from `html` when that part is empty or junk.
pub fn clean_body(text: &str, html: Option<&str>) -> String {
    let cleaned = clean_text(text);
    match html {
        Some(h) if is_junk(&cleaned) => {
            let derived = html_to_clean_text(h);
            if derived.is_empty() || (is_junk(&derived) && derived.len() < cleaned.len()) { cleaned } else { derived }
        }
        _ => cleaned,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped like Google's "Sign in with Google" notice: conditional comments, `<https://…>` wrappers
    /// before and after their anchor text, and the same block sent twice in a row.
    const BLOCK: &str = "[image: Google]\r\n\
\r\n\
Postman was granted access to your Google Account\r\n\
\r\n\
someone@example.com\r\n\
\r\n\
If you did not grant access, you should check this activity and secure your account.\r\n\
<https://accounts.google.com/AccountChooser?Email=someone@example.com&continue=https://myaccount.google.com/alert/nt/1>\r\n\
Check activity\r\n\
<https://accounts.google.com/AccountChooser?Email=someone@example.com&continue=https://myaccount.google.com/alert/nt/2>  \r\n\
\r\n\
Open <https://c.gle/ABC123>Postman  \r\n\
<https://c.gle/DEF456>Google Account settings\r\n\
\r\n\
You received this email to let you know about important changes to your Google Account and services.";

    fn google_notice() -> String {
        format!(
            "<!--[if !mso]><!-->\r\n{BLOCK}\r\n<![endif]-->\r\n<!--[if false]>Hidden Outlook-only copy<![endif]-->\r\n\r\n\r\n\r\n\r\n{BLOCK}\r\n\r\n\r\n\u{a9} 2026 Google LLC, 1600 Amphitheatre Parkway, Mountain View, CA 94043, USA\r\n"
        )
    }

    #[test]
    fn cleans_google_security_notice() {
        let out = clean_text(&google_notice());
        assert!(!out.contains("<!--") && !out.contains("[if") && !out.contains("endif") && !out.contains("Hidden Outlook"), "{out}");
        assert!(!out.contains("<https"), "{out}");
        assert!(out.contains("Postman https://c.gle/ABC123"), "anchor text then bare url: {out}");
        assert!(out.contains("Google Account settings https://c.gle/DEF456"), "{out}");
        assert!(out.contains("Check activity https://accounts.google.com/AccountChooser?Email=someone@example.com&continue=https://myaccount.google.com/alert/nt/2"), "{out}");
        assert!(out.contains("secure your account. https://accounts.google.com"), "{out}");
        assert!(out.contains("Postman was granted access to your Google Account\n\nsomeone@example.com"), "{out}");
        assert!(!out.contains("\n\n\n"), "no runs of blank lines: {out}");
        assert!(out.lines().all(|l| l == l.trim_end()), "no trailing spaces");
        assert_eq!(out.matches("Postman was granted access to your Google Account").count(), 1, "{out}");
        assert_eq!(out.matches("You received this email to let you know").count(), 1, "{out}");
        assert!(out.contains("\u{a9} 2026 Google LLC"));
    }

    #[test]
    fn is_idempotent() {
        let once = clean_text(&google_notice());
        assert_eq!(clean_text(&once), once);
    }

    #[test]
    fn keeps_plain_text_and_angle_addresses() {
        let t = "Hi Priya,\n\nPlease ping John <john@acme.com> or see <https://acme.com/a>.\n\nThanks";
        assert_eq!(clean_text(t), "Hi Priya,\n\nPlease ping John <john@acme.com> or see https://acme.com/a.\n\nThanks");
    }

    #[test]
    fn strips_stray_tags_and_decodes() {
        assert_eq!(clean_text("Hello&nbsp;<b>there</b><br>Bye &amp; thanks"), "Hello there\nBye & thanks");
        assert_eq!(clean_text("a\u{200c}\u{200c}b"), "ab");
    }

    #[test]
    fn keeps_short_repeats_but_drops_long_ones() {
        assert_eq!(clean_text("Thanks\n\nThanks"), "Thanks\n\nThanks");
        let block = "Your verification code is 123456 and expires soon.";
        assert_eq!(clean_text(&format!("{block}\n\n{block}\n\nBye")), format!("{block}\n\nBye"));
    }

    #[test]
    fn derives_text_from_html_when_text_is_junk() {
        let html = "<html><head><style>p{color:red}</style></head><body><!--[if mso]><p>Outlook only</p><![endif]-->\
<p>Your invoice <b>#123</b> is ready.</p><p><a href=\"https://pay.example/inv/123\">Pay now</a></p></body></html>";
        let out = clean_body("[image: logo]\n\n<https://x.example/t.png>", Some(html));
        assert_eq!(out, "Your invoice #123 is ready.\n\nPay now https://pay.example/inv/123", "{out}");
        assert_eq!(clean_body("", Some("<p>Only html here, with enough words.</p>")), "Only html here, with enough words.");
        let real = "Hello, this is a perfectly fine plain text body.";
        assert_eq!(clean_body(real, Some("<p>other</p>")), real);
        assert_eq!(clean_body("", None), "");
    }
}
