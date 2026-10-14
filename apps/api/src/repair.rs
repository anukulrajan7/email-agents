//! One automatic repair for a send the review stopped over an unsupported claim: the sentence the review
//! pointed at is dropped and replaced by "We'll confirm the details shortly." Sensitive claims (money,
//! legal, dates) are never repaired: they go to the owner as one review task.

use std::collections::HashSet;

use crate::{
    db::{new_id, now_ms},
    http::AppState,
    incidents::{Kind, NewIncident},
    replies::ReplyContext,
    send_safety::SafetyReport,
};

const STOP: &[&str] = &[
    "that", "this", "with", "from", "which", "have", "provided", "notes", "team", "email", "supported", "support", "promises", "promise",
    "claims", "claim", "states", "mentions", "unsupported", "reply", "sender", "about", "their", "there", "would", "could", "into", "than",
];

/// The review's own words about the claim: the text in the last parentheses of its message.
fn reason_of(report: &SafetyReport) -> Option<String> {
    let m = &report.blocking.iter().find(|i| i.code == "unsupported_promise")?.message;
    let open = m.rfind('(')?;
    let close = m.rfind(')')?;
    (close > open).then(|| m[open + 1..close].to_string())
}

fn words(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 4 && !STOP.contains(w))
        .map(str::to_string)
        .collect()
}

fn sentences(paragraph: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in paragraph.char_indices() {
        if matches!(c, '.' | '!' | '?') && paragraph[i + 1..].chars().next().is_none_or(char::is_whitespace) {
            out.push(&paragraph[start..=i]);
            start = i + 1;
        }
    }
    if !paragraph[start..].trim().is_empty() {
        out.push(&paragraph[start..]);
    }
    out
}

/// The repaired body, or `None` when the claim can't be located safely or is sensitive.
pub fn repaired_body(body: &str, report: &SafetyReport) -> Option<String> {
    if report.blocking.len() != 1 {
        return None;
    }
    let reason = reason_of(report)?;
    if crate::gap_resolver::is_hard(&reason) {
        return None;
    }
    let wanted = words(&reason);
    let mut paras: Vec<String> = body.split("\n\n").map(str::to_string).collect();
    if paras.len() < 3 {
        return None;
    }
    let last = paras.len() - 1;
    let mut found_in: Vec<(usize, usize, usize)> = Vec::new();
    for (pi, para) in paras.iter().enumerate().take(last).skip(1) {
        for (si, s) in sentences(para).iter().enumerate() {
            let hits = words(s).intersection(&wanted).count();
            if hits >= 2 {
                found_in.push((hits, pi, si));
            }
        }
    }
    let top = found_in.iter().map(|f| f.0).max()?;
    let mut removed = Vec::new();
    for (pi, para) in paras.iter_mut().enumerate() {
        let parts: Vec<String> = sentences(para).iter().map(|s| s.to_string()).collect();
        let kept: Vec<&String> = parts
            .iter()
            .enumerate()
            .filter(|(si, s)| {
                let drop = found_in.iter().any(|(h, p, i)| *h == top && *p == pi && i == si);
                if drop {
                    removed.push((*s).clone());
                }
                !drop
            })
            .map(|(_, s)| s)
            .collect();
        if kept.len() != parts.len() {
            *para = kept.iter().map(|s| s.trim()).collect::<Vec<_>>().join(" ");
        }
    }
    if removed.iter().any(|s| crate::gap_resolver::is_hard(s)) {
        return None;
    }
    paras.retain(|p| !p.trim().is_empty());
    let at = paras.len().saturating_sub(1);
    paras.insert(at, "We'll confirm the details shortly.".to_string());
    Some(paras.join("\n\n"))
}

/// Writes the repair, records a low-severity incident and the audit trail. `true` when the draft changed.
pub async fn apply(state: &AppState, ws: &str, ctx: &ReplyContext, report: &SafetyReport) -> sqlx::Result<bool> {
    let Some(body) = repaired_body(&ctx.body_text, report) else { return Ok(false) };
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let changed = sqlx::query(
        "UPDATE drafts SET body_text = ?1, unanswered_json = '[\"details\"]', updated_at = ?2
         WHERE id = ?3 AND workspace_id = ?4 AND body_text = ?5 AND send_at IS NULL AND status != 'sent'",
    )
    .bind(&body)
    .bind(now_ms())
    .bind(&ctx.draft_id)
    .bind(ws)
    .bind(&ctx.body_text)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if changed {
        crate::audit::record(
            &mut tx,
            crate::audit::Entry {
                workspace_id: ws,
                actor: crate::audit::Actor::System,
                action: "draft.auto_repaired",
                entity_type: "draft",
                entity_id: &ctx.draft_id,
                message_id: Some(&ctx.message_id),
                metadata: serde_json::json!({ "code": "unsupported_promise" }),
            },
        )
        .await?;
    }
    tx.commit().await?;
    if changed {
        crate::incidents::record(
            &state.db,
            NewIncident::new(ws, Kind::UnsupportedClaim, serde_json::json!({ "repaired": true }))
                .severity("low")
                .message(Some(&ctx.message_id))
                .dedupe(format!("repaired:{}", ctx.draft_id)),
        )
        .await;
    }
    Ok(changed)
}

/// One open review task for an automatic send that could not be repaired. Idempotent per draft.
pub async fn hand_off(state: &AppState, ws: &str, ctx: &ReplyContext, report: &SafetyReport) -> sqlx::Result<()> {
    if !report.blocking.iter().any(|i| i.code == "unsupported_promise") {
        return Ok(());
    }
    let title = format!("I couldn't send this reply safely - please read it and send it yourself: {}", crate::sources::normalize::truncate_chars(&ctx.subject, 120));
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM review_items WHERE workspace_id = ?1 AND message_id = ?2 AND kind = 'agent_task' AND title = ?3 AND status = 'open')")
        .bind(ws)
        .bind(&ctx.message_id)
        .bind(&title)
        .fetch_one(&state.db)
        .await?;
    if exists {
        return Ok(());
    }
    let owner: Option<String> = sqlx::query_scalar("SELECT connected_by_user_id FROM email_sources WHERE id = ?1").bind(&ctx.source_id).fetch_optional(&state.db).await?.flatten();
    sqlx::query(
        "INSERT INTO review_items (id, workspace_id, message_id, workflow_run_id, kind, title, status, assigned_user_id, created_at)
         VALUES (?1, ?2, ?3, NULL, 'agent_task', ?4, 'open', ?5, ?6)",
    )
    .bind(new_id("rev"))
    .bind(ws)
    .bind(&ctx.message_id)
    .bind(title)
    .bind(owner)
    .bind(now_ms())
    .execute(&state.db)
    .await?;
    Ok(())
}
