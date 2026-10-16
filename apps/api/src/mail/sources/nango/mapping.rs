use super::*;

pub struct MappedRecord {
    pub email: NormalizedEmail,
    pub thread_id: Option<String>,
    pub deleted: bool,
    pub cursor: Option<String>,
}

fn str_of(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| match v.get(*k) {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    })
}

fn addresses(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::String(s)) => s.split(',').map(normalize::address).filter(|a| !a.is_empty()).collect(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| i.as_str().map(str::to_string).or_else(|| str_of(i, &["email", "address"])))
            .map(|a| normalize::address(&a))
            .collect(),
        _ => vec![],
    }
}

/// Maps a Nango Gmail record. Field names differ between template versions, so this
/// accepts the common spellings; anything without an ID is skipped.
pub fn map_gmail_record(rec: &Value, max_body_chars: usize) -> Option<MappedRecord> {
    let id = str_of(rec, &["id"])?;
    let meta = rec.get("_nango_metadata");
    let deleted = meta.is_some_and(|m| {
        m.get("deleted_at").is_some_and(|d| !d.is_null()) || m.get("last_action").and_then(Value::as_str) == Some("DELETED")
    });
    let cursor = meta.and_then(|m| m.get("cursor")).and_then(Value::as_str).map(str::to_string);

    // Nango's catalog `messages` sync stores Gmail-native records: headers live in
    // payload.headers and there is no body (only `snippet`) unless enriched later.
    let header = |name: &str| gmail_header(rec.get("payload"), name);
    let raw_body = str_of(rec, &["body", "text", "bodyText", "snippet"]).unwrap_or_default();
    let looks_html = raw_body.contains("</") || raw_body.contains("<br");
    let body_text = if looks_html { normalize::html_to_text(&raw_body) } else { raw_body.clone() };
    let attachments = rec
        .get("attachments")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|a| AttachmentMeta {
                    filename: normalize::truncate_chars(&str_of(a, &["filename", "name"]).unwrap_or_else(|| "attachment".into()), 255),
                    media_type: str_of(a, &["mimeType", "mime_type", "contentType", "content_type"]),
                    size_bytes: a.get("size").and_then(Value::as_i64),
                    provider_ref: None,
                })
                .collect()
        })
        .unwrap_or_default();

    let email = NormalizedEmail {
        source_message_id: id,
        rfc_message_id: str_of(rec, &["messageId", "message_id", "rfcMessageId"]).or_else(|| header("Message-ID")),
        sender_address: normalize::address(&str_of(rec, &["sender", "from"]).or_else(|| header("From")).unwrap_or_default()),
        sender_name: str_of(rec, &["sender", "from"]).or_else(|| header("From")).as_deref().and_then(normalize::display_name),
        // A connected mailbox sees the real sender; there is no forwarded wrapper to parse.
        original_sender_address: None,
        recipients: match rec.get("recipients").or_else(|| rec.get("to")) {
            Some(v) => addresses(Some(v)),
            None => [header("To"), header("Cc")]
                .into_iter()
                .flatten()
                .flat_map(|h| addresses(Some(&Value::String(h))))
                .collect(),
        },
        cc: match rec.get("cc") {
            Some(v) => addresses(Some(v)),
            None => header("Cc").map(|h| addresses(Some(&Value::String(h)))).unwrap_or_default(),
        },
        outbound: has_sent_label(rec),
        subject: normalize::truncate_chars(
            str_of(rec, &["subject"])
                .or_else(|| header("Subject"))
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("(no subject)"),
            998,
        ),
        body_text: normalize::truncate_chars(&body_text, max_body_chars),
        body_html: looks_html.then(|| normalize::truncate_chars(&raw_body, max_body_chars)),
        received_at: str_of(rec, &["date", "receivedAt", "received_at", "internalDate"])
            .and_then(|d| parse_iso_ms(&d).or_else(|| d.parse::<i64>().ok()))
            .unwrap_or_else(now_ms),
        attachments,
        // Only a snippet unless an explicit body field was present.
        preview_only: str_of(rec, &["body", "text", "bodyText"]).is_none(),
        bulk_signals: crate::mail::bulk::header_signals(&header, &label_ids(rec)),
        in_reply_to: header("In-Reply-To").as_deref().and_then(crate::mail::headers::in_reply_to),
        references: header("References").map(|r| crate::mail::headers::message_ids(&r)).unwrap_or_default(),
        auth: header("Authentication-Results").map(|a| crate::mail::headers::auth_results(&a)),
    };
    let mut mapped = MappedRecord { email, thread_id: str_of(rec, &["threadId", "thread_id"]), deleted, cursor };
    // If the record already carries MIME parts (full format), use them directly.
    if let Some(payload) = rec.get("payload") {
        apply_gmail_payload(&mut mapped.email, payload, max_body_chars);
    }
    Some(mapped)
}

fn label_ids(rec: &Value) -> Vec<String> {
    rec.get("labelIds").and_then(Value::as_array).map(|l| l.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// Gmail marks mail the mailbox itself sent with the SENT label.
pub(super) fn has_sent_label(rec: &Value) -> bool {
    rec.get("labelIds").and_then(Value::as_array).is_some_and(|l| l.iter().any(|x| x.as_str() == Some("SENT")))
}

fn gmail_header(payload: Option<&Value>, name: &str) -> Option<String> {
    payload?
        .get("headers")?
        .as_array()?
        .iter()
        .find(|h| h.get("name").and_then(Value::as_str).is_some_and(|n| n.eq_ignore_ascii_case(name)))
        .and_then(|h| h.get("value").and_then(Value::as_str))
        .map(str::to_string)
        .filter(|v| !v.trim().is_empty())
}

fn decode_b64url(data: &str) -> Option<String> {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let bytes = URL_SAFE_NO_PAD.decode(data.trim().trim_end_matches('=')).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[derive(Default)]
struct GmailContent {
    text: Option<String>,
    html: Option<String>,
    attachments: Vec<AttachmentMeta>,
}

/// Walks a Gmail `format=full` MIME tree: first text/plain and text/html bodies, plus
/// attachment metadata (parts with a filename). Attachment bytes are never fetched.
fn walk_gmail_part(part: &Value, out: &mut GmailContent) {
    let mime = part.get("mimeType").and_then(Value::as_str).unwrap_or_default();
    let filename = part.get("filename").and_then(Value::as_str).unwrap_or_default();
    let body = part.get("body");
    if !filename.is_empty() {
        out.attachments.push(AttachmentMeta {
            filename: normalize::truncate_chars(filename, 255),
            media_type: (!mime.is_empty()).then(|| mime.to_string()),
            size_bytes: body.and_then(|b| b.get("size")).and_then(Value::as_i64),
            provider_ref: body.and_then(|b| b.get("attachmentId")).and_then(Value::as_str).map(str::to_string),
        });
    } else if let Some(data) = body.and_then(|b| b.get("data")).and_then(Value::as_str) {
        if mime == "text/plain" && out.text.is_none() {
            out.text = decode_b64url(data);
        } else if mime == "text/html" && out.html.is_none() {
            out.html = decode_b64url(data);
        }
    }
    for child in part.get("parts").and_then(Value::as_array).into_iter().flatten() {
        walk_gmail_part(child, out);
    }
}

/// Fills body/attachments (and any missing headers) from a Gmail payload.
pub fn apply_gmail_payload(email: &mut NormalizedEmail, payload: &Value, max_body_chars: usize) {
    let mut content = GmailContent::default();
    walk_gmail_part(payload, &mut content);
    match (content.text, content.html) {
        (Some(t), html) if !t.trim().is_empty() => {
            email.body_text = normalize::truncate_chars(&t, max_body_chars);
            email.body_html = html.map(|h| normalize::truncate_chars(&h, max_body_chars));
            email.preview_only = false;
        }
        (_, Some(h)) => {
            email.body_text = normalize::truncate_chars(&normalize::html_to_text(&h), max_body_chars);
            email.body_html = Some(normalize::truncate_chars(&h, max_body_chars));
            email.preview_only = false;
        }
        _ => {}
    }
    if !content.attachments.is_empty() {
        email.attachments = content.attachments;
    }
    crate::mail::bulk::merge(&mut email.bulk_signals, crate::mail::bulk::header_signals(&|n| gmail_header(Some(payload), n), &[]));
    if email.in_reply_to.is_none() {
        email.in_reply_to = gmail_header(Some(payload), "In-Reply-To").as_deref().and_then(crate::mail::headers::in_reply_to);
    }
    if email.references.is_empty() {
        email.references = gmail_header(Some(payload), "References").map(|r| crate::mail::headers::message_ids(&r)).unwrap_or_default();
    }
    if email.auth.is_none() {
        email.auth = gmail_header(Some(payload), "Authentication-Results").map(|a| crate::mail::headers::auth_results(&a));
    }
    if email.sender_address.is_empty()
        && let Some(from) = gmail_header(Some(payload), "From")
    {
        email.sender_address = normalize::address(&from);
        email.sender_name = normalize::display_name(&from);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_nango_catalog_gmail_messages() {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let rec = json!({
            "id": "18c2ab", "threadId": "t9", "snippet": "Please find my timesheet", "historyId": "1",
            "internalDate": "1791900000000",
            "payload": {"headers": [
                {"name": "From", "value": "Priya Sharma <Priya@Acme.com>"},
                {"name": "To", "value": "hr@acme.com"}, {"name": "Cc", "value": "lead@acme.com"},
                {"name": "Subject", "value": "September timesheet"}, {"name": "Message-ID", "value": "<abc@mail>"}
            ]},
            "_nango_metadata": {"deleted_at": null, "last_action": "ADDED", "cursor": "c9"}
        });
        let m = map_gmail_record(&rec, 10_000).unwrap();
        assert_eq!(m.email.sender_address, "priya@acme.com");
        assert_eq!(m.email.sender_name.as_deref(), Some("Priya Sharma"));
        assert_eq!(m.email.recipients, vec!["hr@acme.com", "lead@acme.com"]);
        assert_eq!(m.email.subject, "September timesheet");
        assert_eq!(m.email.received_at, 1_791_900_000_000);
        assert_eq!(m.email.body_text, "Please find my timesheet", "snippet until enriched");

        // Enrichment from format=full: nested multipart with a PDF attachment.
        let full = json!({"mimeType": "multipart/mixed", "headers": [], "parts": [
            {"mimeType": "multipart/alternative", "parts": [
                {"mimeType": "text/plain", "body": {"data": URL_SAFE_NO_PAD.encode("Hi HR,\nAttached: 168 hours.")}},
                {"mimeType": "text/html", "body": {"data": URL_SAFE_NO_PAD.encode("<p>Hi HR</p>")}}
            ]},
            {"mimeType": "application/pdf", "filename": "timesheet.pdf", "body": {"attachmentId": "a1", "size": 4821}}
        ]});
        let mut email = m.email.clone();
        apply_gmail_payload(&mut email, &full, 10_000);
        assert_eq!(email.body_text, "Hi HR,\nAttached: 168 hours.");
        assert_eq!(email.body_html.as_deref(), Some("<p>Hi HR</p>"));
        assert_eq!(email.attachments.len(), 1);
        assert_eq!((email.attachments[0].filename.as_str(), email.attachments[0].size_bytes), ("timesheet.pdf", Some(4821)));
    }

    #[test]
    fn threading_headers_and_what_the_server_concluded_about_the_sender_are_read() {
        let rec = json!({
            "id": "18c3", "threadId": "t2", "snippet": "my id is EMP-1088",
            "payload": {"headers": [
                {"name": "From", "value": "Ravi <ravi@client.test>"}, {"name": "Subject", "value": "Re: timesheet"},
                {"name": "In-Reply-To", "value": "<snd_1@postroom.mail>"},
                {"name": "References", "value": "<gm-1@mail.example> <snd_1@postroom.mail>"},
                {"name": "Authentication-Results", "value": "mx.google.com; dkim=pass; spf=pass; dmarc=fail (p=REJECT)"}
            ]},
            "_nango_metadata": {"deleted_at": null, "last_action": "ADDED", "cursor": "c2"}
        });
        let m = map_gmail_record(&rec, 10_000).unwrap();
        assert_eq!(m.email.in_reply_to.as_deref(), Some("<snd_1@postroom.mail>"));
        assert_eq!(m.email.references, ["<gm-1@mail.example>", "<snd_1@postroom.mail>"]);
        assert!(crate::mail::headers::failed(m.email.auth.as_ref().unwrap()), "dmarc failed");
        let plain = map_gmail_record(&json!({"id": "x", "snippet": "hi", "payload": {"headers": []}}), 10_000).unwrap();
        assert_eq!((plain.email.in_reply_to, plain.email.references, plain.email.auth), (None, vec![], None));
    }

    #[test]
    fn maps_gmail_records_tolerantly() {
        let rec = json!({
            "id": "18c2", "threadId": "t1", "sender": "Priya <Priya@Acme.com>",
            "recipients": "hr@acme.com, boss@acme.com", "date": "2026-10-10T09:00:00Z",
            "subject": "Leave next week", "body": "<p>Hi</p><p>Can I take Friday off?</p>",
            "attachments": [{"filename":"note.pdf","mimeType":"application/pdf","size":12}],
            "_nango_metadata": {"deleted_at": null, "last_action": "ADDED", "cursor": "c1"}
        });
        let m = map_gmail_record(&rec, 10_000).unwrap();
        assert_eq!(m.email.sender_address, "priya@acme.com");
        assert_eq!(m.email.recipients, vec!["hr@acme.com", "boss@acme.com"]);
        assert_eq!(m.email.body_text, "Hi\n\nCan I take Friday off?");
        assert_eq!(m.thread_id.as_deref(), Some("t1"));
        assert_eq!((m.deleted, m.cursor.as_deref()), (false, Some("c1")));
        assert_eq!(m.email.attachments[0].media_type.as_deref(), Some("application/pdf"));

        let gone = json!({"id": 7, "_nango_metadata": {"deleted_at": "2026-10-11T00:00:00Z"}});
        let g = map_gmail_record(&gone, 100).unwrap();
        assert!(g.deleted);
        assert_eq!(g.email.source_message_id, "7");
        assert!(map_gmail_record(&json!({"subject":"no id"}), 100).is_none());
    }
}
