use super::*;

struct Answer {
    who: String,
    text: String,
    citation: Option<Citation>,
    links: Value,
    warning: Option<&'static str>,
}

async fn read_answer(state: &AppState, w: &Watch, what: &str, reference: &str) -> sqlx::Result<Option<Answer>> {
    match what {
        "email" => {
            let row: Option<(String, Option<String>, String, String)> = sqlx::query_as(
                "SELECT sender_address, sender_name, subject, body_text FROM email_messages WHERE id = ?1 AND workspace_id = ?2 AND suppressed = 0",
            )
            .bind(reference)
            .bind(&w.workspace_id)
            .fetch_optional(&state.db)
            .await?;
            let signals: Option<String> = sqlx::query_scalar("SELECT bulk_signals FROM email_messages WHERE id = ?1").bind(reference).fetch_optional(&state.db).await?;
            if row.as_ref().is_some_and(|(address, _, subject, _)| automatic(address, subject, signals.as_deref().unwrap_or("[]")).is_some()) {
                return Ok(None);
            }
            Ok(row.map(|(address, name, subject, body)| {
                let clean = knowledge::clean_body(&body);
                Answer {
                    who: name.filter(|n| !n.trim().is_empty()).unwrap_or(address),
                    warning: injection::scan(&clean),
                    text: plain_answer(&clean, ANSWER_WORDS),
                    citation: Some(Citation { message_id: reference.to_string(), subject, attachment_id: None, attachment_name: None }),
                    links: json!([{ "kind": "message", "id": reference, "label": "Open their reply" }]),
                }
            }))
        }
        "in_app" => {
            let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
                "SELECT m.body, u.name, u.email FROM in_app_messages m LEFT JOIN users u ON u.id = m.sender_user_id
                 WHERE m.id = ?1 AND m.workspace_id = ?2 AND m.recipient_user_id = ?3",
            )
            .bind(reference)
            .bind(&w.workspace_id)
            .bind(&w.user_id)
            .fetch_optional(&state.db)
            .await?;
            Ok(row.map(|(body, name, email)| Answer {
                who: name.filter(|n| !n.trim().is_empty()).or(email).unwrap_or_else(|| w.counterpart.clone()),
                warning: injection::scan(&body),
                text: plain_answer(&body, ANSWER_WORDS),
                citation: None,
                links: json!([{ "kind": "inbox", "id": reference, "label": "Open the reply" }]),
            }))
        }
        "case" => {
            let row: Option<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
                "SELECT status, status_note, failure_reason, title FROM cases WHERE id = ?1 AND workspace_id = ?2",
            )
            .bind(reference)
            .bind(&w.workspace_id)
            .fetch_optional(&state.db)
            .await?;
            Ok(row.map(|(status, note, failure, _)| Answer {
                who: w.counterpart.clone(),
                warning: None,
                text: match status.as_str() {
                    "done" => note.unwrap_or_else(|| "It is finished.".into()),
                    _ => failure.or(note).unwrap_or_else(|| "It could not be finished.".into()),
                },
                citation: None,
                links: json!([{ "kind": "case", "id": reference, "label": "Open the case" }]),
            }))
        }
        _ => Ok(None),
    }
}

/// `WATCH_REPLY`: post the answer into the person's Meera conversation. The status change and the report are one
/// transaction, so a failure is retried instead of dropped.
pub async fn reply_job(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let (ws, id) = job_fields(payload)?;
    let what = payload.get("what").and_then(Value::as_str).unwrap_or_default();
    let reference = payload.get("ref").and_then(Value::as_str).unwrap_or_default();
    let Some(w) = get(state, &ws, &id).await? else { return Ok(()) };
    if what == "bounce" {
        return bounce_job(state, &w, reference).await;
    }
    if w.status == "closed" || (w.status == "replied" && w.kind != "ticket") {
        return Ok(());
    }
    let Some(answer) = read_answer(state, &w, what, reference).await? else { return Ok(()) };
    let resolved = w.kind == "ticket" && crate::alerts::messaging::support::looks_resolved(&answer.text);
    let mut text = match w.kind.as_str() {
        "ticket" => format!("{} replied about “{}”: {}", answer.who, w.title, answer.text),
        "desk" => format!("{} finished “{}”: {}", answer.who, w.title, answer.text),
        _ => format!("{} answered your question “{}”: {}", answer.who, w.title, answer.text),
    };
    if answer.warning.is_some() {
        text.push_str(" Their message also contains text that tries to give me instructions, so I ignored that part.");
    }
    if resolved {
        text.push_str(" That sounds resolved, so I marked it done. Tell me if it is not.");
    }
    let now = now_ms();
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let updated = sqlx::query(
        "UPDATE watches SET status = 'replied', answer_text = ?1, answer_message_id = ?2, answered_at = ?3, last_update_at = ?3
         WHERE id = ?4 AND status != 'closed'",
    )
    .bind(&answer.text)
    .bind(reference)
    .bind(now)
    .bind(&id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if updated == 0 {
        return Ok(());
    }
    if w.kind == "ticket" {
        sqlx::query("UPDATE support_tickets SET status = ?1, last_summary = ?2, last_update_at = ?3 WHERE workspace_id = ?4 AND watch_id = ?5")
            .bind(if resolved { "resolved" } else { "open" })
            .bind(sentence(&answer.text, 300))
            .bind(now)
            .bind(&ws)
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    }
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::System,
            action: "message.answer_reported",
            entity_type: "watch",
            entity_id: &id,
            message_id: (what == "email").then_some(reference),
            metadata: json!({ "kind": w.kind, "what": what, "resolved": resolved, "instructions_ignored": answer.warning.is_some() }),
        },
    )
    .await?;
    let citations: Vec<Citation> = answer.citation.into_iter().collect();
    crate::alerts::messaging::report_in(&mut tx, &ws, &w.user_id, &text, &citations, &answer.links).await?;
    let headline = if w.kind == "ticket" { format!("{} replied", w.counterpart) } else { format!("{} answered", answer.who) };
    crate::alerts::messaging::inapp::notify_in(&mut tx, &ws, &w.user_id, w.agent_id.as_deref(), &headline, &sentence(&text, 300), json!({ "kind": "meera" }), false).await?;
    tx.commit().await?;
    Ok(())
}

/// The email bounced: tell the person. The watch keeps waiting (a bounce is not an answer).
async fn bounce_job(state: &AppState, w: &Watch, reference: &str) -> Result<(), JobError> {
    if w.status == "closed" {
        return Ok(());
    }
    let text = format!(
        "Your message to {} bounced: “{}”. Nobody has read it, so the address may be wrong or the mailbox full.",
        w.counterpart,
        sentence(&w.title, 80)
    );
    let now = now_ms();
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    if w.kind == "ticket" {
        sqlx::query("UPDATE support_tickets SET status = 'open', last_summary = 'The email bounced.', last_update_at = ?1 WHERE workspace_id = ?2 AND watch_id = ?3")
            .bind(now)
            .bind(&w.workspace_id)
            .bind(&w.id)
            .execute(&mut *tx)
            .await?;
    }
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &w.workspace_id,
            actor: Actor::System,
            action: "message.bounced",
            entity_type: "watch",
            entity_id: &w.id,
            message_id: Some(reference),
            metadata: json!({ "kind": w.kind }),
        },
    )
    .await?;
    crate::alerts::messaging::report_in(&mut tx, &w.workspace_id, &w.user_id, &text, &[], &json!([{ "kind": "message", "id": reference, "label": "Open the bounce notice" }])).await?;
    let headline = format!("Your message to {} bounced", w.counterpart);
    crate::alerts::messaging::inapp::notify_in(&mut tx, &w.workspace_id, &w.user_id, w.agent_id.as_deref(), &headline, &text, json!({ "kind": "message", "id": reference }), false).await?;
    tx.commit().await?;
    Ok(())
}
