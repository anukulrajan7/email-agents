use super::*;

/// Longest subject we put on a reply (characters).
pub const MAX_SUBJECT_CHARS: usize = 250;

/// One header line: CR/LF and other control characters (from untrusted inbound mail) become spaces.
pub fn header_safe(value: &str) -> String {
    value.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string()
}

pub fn reply_subject(subject: &str) -> String {
    let s = header_safe(subject);
    let s = if s.to_lowercase().starts_with("re:") { s } else { format!("Re: {s}") };
    crate::mail::sources::normalize::truncate_chars(&s, MAX_SUBJECT_CHARS)
}

static MSG_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^<[^\s<>]+>$").expect("valid regex"));

/// `In-Reply-To` / `References` value, only when it is a well-formed `<id>`; anything else is dropped.
pub fn safe_message_id(raw: &str) -> Option<String> {
    let v = raw.trim();
    (v.len() <= 998 && MSG_ID.is_match(v)).then(|| v.to_string())
}

/// RFC 5322 reply, base64url-encoded for Gmail's `raw` field.
pub fn build_raw(from: &str, ctx_to: &str, subject: &str, in_reply_to: Option<&str>, body: &str) -> AppResult<String> {
    build_raw_with_id(from, ctx_to, subject, in_reply_to, None, body)
}

/// Like `build_raw`, with our own `Message-ID` (the send attempt id) so the send can be found in Gmail later.
pub fn build_raw_with_id(
    from: &str,
    ctx_to: &str,
    subject: &str,
    in_reply_to: Option<&str>,
    message_id: Option<&str>,
    body: &str,
) -> AppResult<String> {
    let bad = |e: String| AppError::BadRequest(e);
    let from: Mailbox = header_safe(from).parse().map_err(|_| bad("connected mailbox address is invalid".into()))?;
    let to: Mailbox = header_safe(ctx_to).parse().map_err(|_| bad("the original sender's address is invalid".into()))?;
    let mut builder = Message::builder().from(from).to(to).subject(reply_subject(subject));
    if let Some(id) = in_reply_to.and_then(safe_message_id) {
        builder = builder.in_reply_to(id.clone()).references(id);
    }
    if let Some(id) = message_id.and_then(safe_message_id) {
        builder = builder.message_id(Some(id));
    }
    let msg = builder
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_string())
        .map_err(|e| AppError::Internal(anyhow::anyhow!("building reply: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(msg.formatted()))
}

pub async fn mailbox_address(state: &AppState, conn: &str, pck: &str) -> AppResult<String> {
    let profile = state.nango.gmail_call(Method::GET, conn, pck, "profile", None).await?;
    profile
        .get("emailAddress")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(AppError::Unavailable)
}

pub(super) async fn raw_for(state: &AppState, ctx: &ReplyContext, conn: &str, pck: &str, message_id: Option<&str>) -> AppResult<Value> {
    if ctx.body_text.trim().is_empty() {
        return Err(AppError::BadRequest("the draft is empty".into()));
    }
    let from = mailbox_address(state, conn, pck).await?;
    let raw = build_raw_with_id(&from, &ctx.sender_address, &ctx.subject, ctx.rfc_message_id.as_deref(), message_id, &ctx.body_text)?;
    let mut message = json!({ "raw": raw });
    if let Some(t) = &ctx.provider_thread_id {
        message["threadId"] = json!(t);
    }
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(raw: &str) -> String {
        String::from_utf8(URL_SAFE_NO_PAD.decode(raw).unwrap()).unwrap()
    }

    #[test]
    fn builds_threaded_reply() {
        assert_eq!(reply_subject("Timesheet"), "Re: Timesheet");
        assert_eq!(reply_subject("RE: Timesheet"), "RE: Timesheet");
        let raw = build_raw("me@acme.com", "Priya <priya@acme.com>", "Timesheet", Some("<abc@mail>"), "Thanks!").unwrap();
        let text = decode(&raw);
        assert!(text.contains("Subject: Re: Timesheet"));
        assert!(text.contains("In-Reply-To: <abc@mail>"));
        assert!(text.contains("References: <abc@mail>"));
        assert!(text.contains("To: Priya <priya@acme.com>"));
        assert!(text.ends_with("Thanks!") || text.contains("\r\n\r\nThanks!"));
    }

    #[test]
    fn headers_from_inbound_mail_are_sanitized() {
        // CR/LF in the inbound subject can't inject headers.
        let evil = "Hello\r\nBcc: attacker@evil.com\r\nX-Evil: 1";
        assert!(!reply_subject(evil).contains('\n') && !reply_subject(evil).contains('\r'));
        let raw = build_raw("me@acme.com", "priya@acme.com", evil, Some("<ok@id>\r\nBcc: x@evil.com"), "Body text here").unwrap();
        let text = decode(&raw);
        assert!(!text.contains("\r\nBcc:") && !text.contains("\r\nX-Evil:"), "{text}");
        assert!(!text.contains("In-Reply-To"), "an invalid In-Reply-To is dropped");
        // Subject is capped.
        assert_eq!(reply_subject(&"x".repeat(1_000)).chars().count(), MAX_SUBJECT_CHARS);
        // Message-ID validation.
        assert_eq!(safe_message_id(" <a.b@c> "), Some("<a.b@c>".into()));
        for bad in ["a@b", "<a b@c>", "<a@b>\r\nX: y", "<<a@b>>", "<>", ""] {
            assert_eq!(safe_message_id(bad), None, "{bad:?}");
        }
        // Our attempt Message-ID is set.
        let raw = build_raw_with_id("me@acme.com", "p@acme.com", "S", None, Some(&attempt_message_id("snd_1")), "Body").unwrap();
        assert!(decode(&raw).contains("Message-ID: <snd_1@postroom.mail>"));
    }
}
