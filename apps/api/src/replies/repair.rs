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
    // Dropping the claim must leave a real reply behind, not just a greeting and a sign-off.
    let last = paras.len().saturating_sub(1);
    let substantive = paras.iter().enumerate().take(last).skip(1).any(|(_, p)| sentences(p).iter().any(|s| s.trim().chars().count() >= 25));
    if !substantive {
        return None;
    }
    let at = paras.len().saturating_sub(1);
    paras.insert(at, "We'll confirm the details shortly.".to_string());
    Some(paras.join("\n\n"))
}

/// Writes the repair, records a low-severity incident and the audit trail. `true` when the draft changed.
pub async fn apply(state: &AppState, ws: &str, ctx: &ReplyContext, report: &SafetyReport) -> sqlx::Result<bool> {
    let Some(body) = repaired_body(&ctx.body_text, report) else { return Ok(false) };
    // A reply that already promises to confirm an amount, date or commitment is not made sendable by a repair.
    let existing: String = sqlx::query_scalar("SELECT unanswered_json FROM drafts WHERE id = ?1 AND workspace_id = ?2").bind(&ctx.draft_id).bind(ws).fetch_optional(&state.db).await?.unwrap_or_default();
    let mut topics = if existing.is_empty() { vec![] } else { crate::gap_resolver::parse_unanswered(&existing) };
    if topics.iter().any(|t| crate::gap_resolver::is_hard(t)) {
        return Ok(false);
    }
    topics.push("details".to_string());
    topics.dedup();
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let changed = sqlx::query(
        "UPDATE drafts SET body_text = ?1, unanswered_json = ?6, updated_at = ?2
         WHERE id = ?3 AND workspace_id = ?4 AND body_text = ?5 AND send_at IS NULL AND status != 'sent'",
    )
    .bind(&body)
    .bind(now_ms())
    .bind(&ctx.draft_id)
    .bind(ws)
    .bind(&ctx.body_text)
    .bind(serde_json::json!(topics).to_string())
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
    // The mark is unique per draft, so two sends racing to hand off create one task between them.
    let first = sqlx::query("INSERT OR IGNORE INTO handoff_marks (draft_id, created_at) VALUES (?1, ?2)").bind(&ctx.draft_id).bind(now_ms()).execute(&state.db).await?.rows_affected() == 1;
    if !first {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::send_safety::{Issue, SafetyReport};

    fn report(reason: &str) -> SafetyReport {
        SafetyReport {
            blocking: vec![Issue { code: "unsupported_promise", message: format!("The reply may promise something unsupported ({reason})."), fix: None }],
            warnings: vec![],
        }
    }

    #[test]
    fn the_named_sentence_goes_and_a_real_reply_stays() {
        let body = "Hello Priya,\n\nThanks for your note about the timesheet.\n\nZeta Corp is hiring and your profile is a fit for Zeta Corp.\n\nBest regards,\nHR";
        let fixed = repaired_body(body, &report("It promises a hiring company Zeta Corp and a profile fit")).unwrap();
        assert!(!fixed.contains("Zeta") && fixed.contains("Thanks for your note about the timesheet.") && fixed.contains("We'll confirm the details shortly."), "{fixed}");
    }

    #[test]
    fn a_repair_that_leaves_nothing_substantive_is_refused() {
        let body = "Hello Priya,\n\nZeta Corp is hiring and your profile is a fit for Zeta Corp.\n\nBest regards,\nHR";
        assert_eq!(repaired_body(body, &report("It promises a hiring company Zeta Corp and a profile fit")), None);
    }

    #[test]
    fn money_and_dates_are_never_repaired() {
        let body = "Hello Priya,\n\nThanks for your note about the timesheet.\n\nYour payment arrives on Friday for the project.\n\nBest regards,\nHR";
        assert_eq!(repaired_body(body, &report("It promises payment on Friday for the project")), None);
    }
}
