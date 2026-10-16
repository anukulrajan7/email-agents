//! The headers that tie one mail to another and say whether the sender is who they claim to be. Pure parsing, no I/O.

use serde_json::{Map, Value, json};

/// Every `<message-id>` in an `In-Reply-To` or `References` value, in order, without duplicates.
pub fn message_ids(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut rest = raw;
    while let Some(start) = rest.find('<') {
        let Some(len) = rest[start..].find('>') else { break };
        let id = rest[start..start + len + 1].trim();
        if id.len() > 2 && id.len() <= 998 && !id.contains(char::is_whitespace) && !out.iter().any(|o| o == id) {
            out.push(id.to_string());
        }
        rest = &rest[start + len + 1..];
    }
    out
}

/// The `In-Reply-To` message-id (the first one) of a header value.
pub fn in_reply_to(raw: &str) -> Option<String> {
    message_ids(raw).into_iter().next()
}

/// What `Authentication-Results` says about each of dmarc, spf and dkim: `{"dmarc": "pass", "spf": "fail"}`. Only results the
/// server wrote are kept; anything unrecognised is ignored.
pub fn auth_results(raw: &str) -> Value {
    let mut out = Map::new();
    for part in raw.split(';') {
        let part = part.trim();
        for method in ["dmarc", "spf", "dkim"] {
            let Some(after) = part.strip_prefix(method).and_then(|r| r.trim_start().strip_prefix('=')) else { continue };
            let result: String = after.trim_start().chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_lowercase();
            if matches!(result.as_str(), "pass" | "fail" | "softfail" | "neutral" | "none" | "temperror" | "permerror" | "policy") {
                out.entry(method.to_string()).or_insert(json!(result));
            }
        }
    }
    Value::Object(out)
}

/// The server says the sender's address is forged: dmarc, spf or dkim failed outright. A missing or soft result is not proof.
pub fn failed(auth: &Value) -> bool {
    ["dmarc", "spf", "dkim"].iter().any(|m| auth.get(m).and_then(Value::as_str) == Some("fail"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_ids_are_read_in_order_without_repeats() {
        assert_eq!(message_ids("<a@x> <b@x>\r\n <a@x>"), ["<a@x>", "<b@x>"]);
        assert_eq!(in_reply_to("  <snd_1@postroom.mail> "), Some("<snd_1@postroom.mail>".to_string()));
        assert!(message_ids("no ids here").is_empty() && message_ids("<broken").is_empty() && message_ids("<>").is_empty());
    }

    #[test]
    fn auth_results_keep_each_method_once() {
        let a = auth_results("mx.google.com; dkim=pass header.i=@x.test; spf=fail (sender mismatch) smtp.mailfrom=x.test; dmarc=pass (p=NONE) header.from=x.test");
        assert_eq!(a, json!({"dkim": "pass", "spf": "fail", "dmarc": "pass"}));
        assert!(failed(&a));
        assert!(!failed(&auth_results("mx.google.com; spf=softfail; dkim=none")) && !failed(&json!({})));
        assert!(failed(&auth_results("dmarc=FAIL (p=reject)")));
    }
}
