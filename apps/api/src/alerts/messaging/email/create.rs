use super::*;

pub(super) fn issue(code: &'static str, message: impl Into<String>, fix: &str) -> Issue {
    Issue { code, message: message.into(), fix: (!fix.is_empty()).then(|| fix.to_string()) }
}

/// The checks every new email passes before it is held, and again before it goes out.
pub fn check(to: &str, subject: &str, body: &str, own: &[String]) -> SafetyReport {
    let mut blocking = vec![];
    let to = bare_address(to);
    if !valid_address(&to) {
        blocking.push(issue("invalid_recipient", format!("\"{to}\" isn't a valid email address."), "Check the address."));
    } else if own.iter().any(|o| bare_address(o) == to) {
        blocking.push(issue("own_mailbox", "This email would go to your own mailbox.", "Pick another recipient."));
    } else if send_safety::is_automated_address(&to) {
        blocking.push(issue("automated_recipient", format!("{to} is an automated address; nobody will read it."), "Find a person's address."));
    }
    let subject = header_safe(subject);
    if subject.is_empty() || subject.chars().count() > MAX_SUBJECT_CHARS {
        blocking.push(issue("bad_subject", format!("The subject must be 1 to {MAX_SUBJECT_CHARS} characters."), "Write a short subject."));
    }
    blocking.extend(send_safety::check_gaps(body));
    blocking.extend(send_safety::check_placeholders(body));
    blocking.extend(send_safety::check_length(body));
    SafetyReport { blocking, ..Default::default() }
}

pub struct NewEmail<'a> {
    pub ws: &'a str,
    pub owner_user_id: Option<&'a str>,
    pub agent_id: Option<&'a str>,
    pub to_address: &'a str,
    pub to_label: &'a str,
    pub kind: Kind,
    pub subject: &'a str,
    pub body: &'a str,
    /// `agent` or `person`: a person's own words are never sent on their own.
    pub authored_by: &'a str,
    pub approval_reason: Option<&'a str>,
    pub in_reply_to: Option<&'a str>,
    pub provider_thread_id: Option<&'a str>,
}

/// The mailbox and the checks for a new email. Nothing is stored.
pub async fn validate(state: &AppState, new: &NewEmail<'_>) -> AppResult<Mailbox> {
    let mailbox = mailbox_for(state, new.ws, new.owner_user_id).await?;
    let owner_email: Option<String> = match new.owner_user_id {
        Some(u) => sqlx::query_scalar("SELECT email FROM users WHERE id = ?1").bind(u).fetch_optional(&state.db).await?,
        None => None,
    };
    let own: Vec<String> = mailbox.address.iter().cloned().chain(owner_email).collect();
    let report = check(new.to_address, new.subject, new.body, &own);
    if report.is_blocked() {
        return Err(report.into_error());
    }
    Ok(mailbox)
}

/// Stores a validated email inside the caller's transaction, waiting for a person's OK or for `schedule`.
pub async fn insert(conn: &mut sqlx::SqliteConnection, new: &NewEmail<'_>, mailbox: &Mailbox) -> sqlx::Result<String> {
    let id = new_id("out");
    let now = now_ms();
    sqlx::query(
        "INSERT INTO outbound_emails (id, workspace_id, source_id, owner_user_id, agent_id, to_address, to_label, subject, body_text,
                                      recipient_kind, authored_by, approval_reason, status, in_reply_to, provider_thread_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'awaiting_approval', ?13, ?14, ?15, ?15)",
    )
    .bind(&id)
    .bind(new.ws)
    .bind(&mailbox.source_id)
    .bind(new.owner_user_id)
    .bind(new.agent_id)
    .bind(bare_address(new.to_address))
    .bind(truncate_chars(new.to_label, 120))
    .bind(header_safe(new.subject))
    .bind(new.body.trim())
    .bind(new.kind.as_str())
    .bind(new.authored_by)
    .bind(new.approval_reason)
    .bind(new.in_reply_to)
    .bind(new.provider_thread_id)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(id)
}

/// Stores the email, waiting for a person's OK or for `schedule`. Nothing is sent.
pub async fn create(state: &AppState, new: NewEmail<'_>) -> AppResult<Outbound> {
    let mailbox = validate(state, &new).await?;
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let id = insert(&mut tx, &new, &mailbox).await?;
    tx.commit().await?;
    get(state, new.ws, &id).await?.ok_or(AppError::NotFound)
}
