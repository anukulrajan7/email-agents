use super::*;

/// The reminder text a nudge would send, and whether the same rules as the original message let it go on its own.
fn reminder_needs_person(subject: &str, body: &str, kinds: &[Kind]) -> Option<String> {
    let injected = crate::alerts::messaging::rules::injection_signal(&[subject, body]);
    crate::alerts::messaging::rules::approval_reason(&crate::alerts::messaging::rules::Facts { recipients: kinds, level: 2, subject, body, attachments_out: false, injected, person_authored: false })
}

struct FollowUp {
    to_address: String,
    to_label: String,
    kind: Kind,
    subject: String,
    body: String,
    reason: String,
    in_reply_to: Option<String>,
    thread: Option<String>,
    mailbox: crate::alerts::messaging::email::Mailbox,
}

enum Plan {
    /// The original was never delivered (stopped, still waiting for an OK, not sent): there is nothing to nudge.
    Close,
    Text(String),
    InApp { recipients: Vec<String>, subject: String, body: String },
    Email(Box<FollowUp>),
}

/// `WATCH_NUDGE`: nobody answered in time. Nudge once, then tell the person. Only a delivered message is nudged;
/// the nudge follows the same approval rules as the original, and the nudge and the report are one transaction.
pub async fn nudge_job(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let (ws, id) = job_fields(payload)?;
    let Some(w) = get(state, &ws, &id).await? else { return Ok(()) };
    if w.status != "waiting" || w.nudged_at.is_some() {
        return Ok(());
    }
    let plan = match w.channel.as_str() {
        "in_app" => plan_in_app(state, &w).await?,
        "email" => plan_email(state, &w).await?,
        _ => Plan::Text(format!("It is still with {}.", w.counterpart)),
    };
    let now = now_ms();
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    if matches!(plan, Plan::Close) {
        let closed = sqlx::query("UPDATE watches SET status = 'closed', last_update_at = ?1 WHERE id = ?2 AND status = 'waiting'").bind(now).bind(&id).execute(&mut *tx).await?.rows_affected();
        if closed == 1 {
            sqlx::query("UPDATE support_tickets SET status = 'resolved', last_summary = 'The message was never delivered.', last_update_at = ?1 WHERE workspace_id = ?2 AND watch_id = ?3")
                .bind(now)
                .bind(&ws)
                .bind(&id)
                .execute(&mut *tx)
                .await?;
            audit::record(
                &mut tx,
                Entry { workspace_id: &ws, actor: Actor::System, action: "message.nudge_skipped", entity_type: "watch", entity_id: &id, message_id: None, metadata: json!({ "reason": "not delivered" }) },
            )
            .await?;
        }
        tx.commit().await?;
        return Ok(());
    }
    let claimed = sqlx::query("UPDATE watches SET status = 'nudged', nudged_at = ?1, last_update_at = ?1 WHERE id = ?2 AND status = 'waiting' AND nudged_at IS NULL")
        .bind(now)
        .bind(&id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        == 1;
    if !claimed {
        return Ok(());
    }
    let hours = w.nudge_hours;
    let waited = if hours % 24 == 0 && hours >= 24 { format!("{} day{}", hours / 24, if hours == 24 { "" } else { "s" }) } else { format!("{hours} hours") };
    let done = match plan {
        Plan::Close => unreachable!("handled above"),
        Plan::Text(t) => t,
        Plan::InApp { recipients, subject, body } => {
            let key = w.thread_key.as_deref();
            for r in &recipients {
                crate::alerts::messaging::inapp::insert(
                    &mut tx,
                    crate::alerts::messaging::inapp::NewMessage {
                        ws: &ws,
                        thread_key: key,
                        sender_user_id: Some(&w.user_id),
                        sender_agent_id: w.agent_id.as_deref(),
                        recipient: r,
                        team_id: None,
                        kind: "message",
                        subject: &subject,
                        body: &body,
                        link: Value::Null,
                        via: "agent",
                        deliver_at: now,
                    },
                )
                .await?;
            }
            format!("I nudged {} once. I will tell you as soon as they answer.", w.counterpart)
        }
        Plan::Email(f) => {
            let new = crate::alerts::messaging::email::NewEmail {
                ws: &ws,
                owner_user_id: Some(&w.user_id),
                agent_id: w.agent_id.as_deref(),
                to_address: &f.to_address,
                to_label: &f.to_label,
                kind: f.kind,
                subject: &f.subject,
                body: &f.body,
                authored_by: "agent",
                approval_reason: Some(&f.reason),
                in_reply_to: f.in_reply_to.as_deref(),
                provider_thread_id: f.thread.as_deref(),
            };
            let out = crate::alerts::messaging::email::insert(&mut tx, &new, &f.mailbox).await?;
            sqlx::query("UPDATE outbound_emails SET watch_id = ?1 WHERE id = ?2").bind(&id).bind(&out).execute(&mut *tx).await?;
            format!("I wrote a short follow-up to {}. It needs your OK before it goes, because {}: open Messages to send it.", w.counterpart, f.reason)
        }
    };
    let text = format!("No answer yet on “{}” after {waited}. {done}", w.title);
    crate::alerts::messaging::report_in(&mut tx, &ws, &w.user_id, &text, &[], &json!([])).await?;
    crate::alerts::messaging::inapp::notify_in(&mut tx, &ws, &w.user_id, w.agent_id.as_deref(), &format!("Still waiting: {}", sentence(&w.title, 80)), &text, json!({ "kind": "meera" }), false).await?;
    audit::record(
        &mut tx,
        Entry { workspace_id: &ws, actor: Actor::System, action: "message.nudged", entity_type: "watch", entity_id: &id, message_id: None, metadata: json!({ "kind": w.kind, "channel": w.channel }) },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn plan_in_app(state: &AppState, w: &Watch) -> Result<Plan, JobError> {
    let Some(key) = w.thread_key.as_deref() else { return Ok(Plan::Close) };
    let recipients: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT recipient_user_id FROM in_app_messages
         WHERE workspace_id = ?1 AND thread_key = ?2 AND sender_user_id = ?3 AND cancelled_at IS NULL AND needs_check = 0 AND deliver_at <= ?4",
    )
    .bind(&w.workspace_id)
    .bind(key)
    .bind(&w.user_id)
    .bind(now_ms())
    .fetch_all(&state.db)
    .await?;
    if recipients.is_empty() {
        return Ok(Plan::Close);
    }
    let subject = format!("Reminder: {}", sentence(&w.title, 120));
    let body = format!("A gentle reminder about this question: {}", w.question);
    let kinds = vec![Kind::Member; recipients.len()];
    let Some(reason) = reminder_needs_person(&subject, &body, &kinds) else { return Ok(Plan::InApp { recipients, subject, body }) };
    if let [only] = recipients.as_slice()
        && let Some(person) = crate::alerts::messaging::recipients::member(&state.db, &w.workspace_id, only).await?
    {
        return follow_up_email(state, w, person, subject, body, &reason, None, None).await;
    }
    Ok(Plan::Text(format!("I did not send a reminder, because {reason}.")))
}

async fn plan_email(state: &AppState, w: &Watch) -> Result<Plan, JobError> {
    let Some(out) = &w.outbound_email_id else { return Ok(Plan::Close) };
    let Some(sent) = crate::alerts::messaging::email::get(state, &w.workspace_id, out).await? else { return Ok(Plan::Close) };
    if sent.status != "sent" {
        return Ok(Plan::Close);
    }
    let person = crate::alerts::messaging::recipients::classify(&state.db, &w.workspace_id, &sent.to_address).await?;
    let subject = format!("Re: {}", sent.subject.trim_start_matches("Re: "));
    let body = format!("Hello,\n\nI am following up on my earlier email “{}”. Could you let me know where this stands?\n\nThank you.", sentence(&sent.subject, 120));
    if let (Kind::Member, Some(uid)) = (person.kind, person.user_id.as_deref()) {
        let reminder_subject = format!("Reminder: {}", sentence(&sent.subject, 120));
        let reminder_body = format!("A gentle reminder about my email “{}”: {}", sentence(&sent.subject, 80), w.question);
        if reminder_needs_person(&reminder_subject, &reminder_body, &[Kind::Member]).is_none() {
            return Ok(Plan::InApp { recipients: vec![uid.to_string()], subject: reminder_subject, body: reminder_body });
        }
    }
    let reason = reminder_needs_person(&subject, &body, &[person.kind]).unwrap_or_else(|| "it is a follow-up to someone outside your workspace".into());
    follow_up_email(state, w, person, subject, body, &reason, sent.rfc_message_id.clone(), sent.provider_thread_id.clone()).await
}

#[allow(clippy::too_many_arguments)]
async fn follow_up_email(
    state: &AppState,
    w: &Watch,
    person: crate::alerts::messaging::recipients::Person,
    subject: String,
    body: String,
    reason: &str,
    in_reply_to: Option<String>,
    thread: Option<String>,
) -> Result<Plan, JobError> {
    let Some(address) = person.address.clone() else { return Ok(Plan::Text("I could not prepare a follow-up automatically.".into())) };
    let check = crate::alerts::messaging::email::NewEmail {
        ws: &w.workspace_id,
        owner_user_id: Some(&w.user_id),
        agent_id: w.agent_id.as_deref(),
        to_address: &address,
        to_label: &person.label,
        kind: person.kind,
        subject: &subject,
        body: &body,
        authored_by: "agent",
        approval_reason: Some(reason),
        in_reply_to: in_reply_to.as_deref(),
        provider_thread_id: thread.as_deref(),
    };
    match crate::alerts::messaging::email::validate(state, &check).await {
        Ok(mailbox) => Ok(Plan::Email(Box::new(FollowUp {
            to_address: address,
            to_label: person.label,
            kind: person.kind,
            subject,
            body,
            reason: reason.to_string(),
            in_reply_to,
            thread,
            mailbox,
        }))),
        Err(AppError::Internal(e)) => Err(JobError::Retry(e.to_string())),
        Err(AppError::Unavailable) => Err(JobError::Retry("unavailable".into())),
        Err(_) => Ok(Plan::Text("I could not prepare a follow-up automatically.".into())),
    }
}
