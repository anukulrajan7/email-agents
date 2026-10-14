//! Candidate watch: a job seeker told "we'll send you matching roles" is remembered. When a new requirement
//! matches them (strong score), they get one email through the normal send path (hold + Stop). At most one
//! per candidate per week, "STOP" opts out, a confidential requirement never names its client, and every
//! step is audited. Staffing pack only.

use serde_json::{Value, json};
use sqlx::SqliteConnection;

use crate::{
    audit::{self, Actor, Entry},
    db::{new_id, now_ms},
    http::AppState,
    jobs::handlers::JobError,
    matching::STRONG_MATCH_SCORE,
    packs, replies,
};

pub const WEEK_MS: i64 = 7 * 86_400_000;

/// Bare, lower-case address of a `Name <a@b>` string.
fn bare(address: &str) -> String {
    let a = address.trim();
    let inner = a.rsplit_once('<').and_then(|(_, r)| r.strip_suffix('>')).unwrap_or(a);
    inner.trim().to_lowercase()
}

/// True when the reply's own first line is just STOP / unsubscribe.
pub fn is_stop(body: &str) -> bool {
    let first = body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    let word: String = first.chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect::<String>().to_lowercase();
    let word = word.trim();
    matches!(word, "stop" | "unsubscribe" | "stop emails" | "please stop" | "opt out") || word.starts_with("stop ") && word.split_whitespace().count() <= 3
}

/// Remembers a sender who replied STOP. Called for every inbound message.
pub async fn opt_out_on_stop(conn: &mut SqliteConnection, ws: &str, sender: &str, body: &str) -> sqlx::Result<()> {
    if !is_stop(body) {
        return Ok(());
    }
    let email = bare(sender);
    let changed = sqlx::query("UPDATE candidate_watch SET opted_out = 1 WHERE workspace_id = ?1 AND email = ?2 AND opted_out = 0")
        .bind(ws)
        .bind(&email)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    if changed > 0 {
        let id: String = sqlx::query_scalar("SELECT id FROM candidate_watch WHERE workspace_id = ?1 AND email = ?2").bind(ws).bind(&email).fetch_one(&mut *conn).await?;
        audit::record(
            &mut *conn,
            Entry {
                workspace_id: ws,
                actor: Actor::System,
                action: "candidate_watch.opted_out",
                entity_type: "candidate_watch",
                entity_id: &id,
                message_id: None,
                metadata: json!({}),
            },
        )
        .await?;
    }
    Ok(())
}

/// Starts watching for this job seeker (staffing only). Uses their candidate record, creating one from the email if needed.
pub async fn enroll(state: &AppState, ws: &str, message_id: &str, sender: &str, subject: &str, body: &str) -> Result<bool, JobError> {
    if !packs::enabled(&state.db, ws, packs::STAFFING).await? {
        return Ok(false);
    }
    let email = bare(sender);
    let existing: Option<String> = sqlx::query_scalar("SELECT id FROM candidates WHERE message_id = ?1").bind(message_id).fetch_optional(&state.db).await?;
    let candidate_id = match existing {
        Some(id) => id,
        None => {
            let c = state.extractor.candidate(subject, body, sender).await?;
            sqlx::query_scalar(
                "INSERT INTO candidates (id, workspace_id, message_id, name, email, skills_json, total_experience, location, notice_days, referred_by, summary, source, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'rules', ?12)
                 ON CONFLICT(message_id) DO UPDATE SET name = excluded.name RETURNING id",
            )
            .bind(new_id("cand"))
            .bind(ws)
            .bind(message_id)
            .bind(crate::sources::normalize::truncate_chars(&c.name, 120))
            .bind(c.email.as_deref().unwrap_or(&email))
            .bind(json!(c.skills).to_string())
            .bind(c.total_experience)
            .bind(&c.location)
            .bind(c.notice_days)
            .bind(&c.referred_by)
            .bind(c.summary.as_deref().map(|s| crate::sources::normalize::truncate_chars(s, 600)))
            .bind(now_ms())
            .fetch_one(&state.db)
            .await?
        }
    };
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let id: String = sqlx::query_scalar(
        "INSERT INTO candidate_watch (id, workspace_id, candidate_id, email, message_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(workspace_id, email) DO UPDATE SET candidate_id = excluded.candidate_id, message_id = excluded.message_id
         RETURNING id",
    )
    .bind(new_id("cwt"))
    .bind(ws)
    .bind(&candidate_id)
    .bind(&email)
    .bind(message_id)
    .bind(now_ms())
    .fetch_one(&mut *tx)
    .await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor: Actor::System,
            action: "candidate_watch.enrolled",
            entity_type: "candidate_watch",
            entity_id: &id,
            message_id: Some(message_id),
            metadata: json!({ "candidate_id": candidate_id }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(true)
}

#[derive(sqlx::FromRow)]
struct Target {
    watch_id: String,
    message_id: String,
    name: String,
    previous: Option<i64>,
    reasons_json: String,
}

fn reasons(json: &str) -> String {
    let all: Vec<Value> = serde_json::from_str(json).unwrap_or_default();
    all.iter()
        .filter(|r| r.get("ok").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|r| r.get("text").and_then(Value::as_str))
        .take(2)
        .collect::<Vec<_>>()
        .join("; ")
}

/// The email text. The client is named only when the requirement is not confidential.
pub fn body_for(name: &str, role: &str, location: Option<&str>, client: Option<&str>, confidential: bool, why: &str, sign_off: &str) -> String {
    let first = name.split_whitespace().next().filter(|n| !n.is_empty() && !matches!(n.to_lowercase().as_str(), "unnamed" | "unknown" | "candidate"));
    let greeting = first.map_or("Hello,".to_string(), |n| format!("Hello {n},"));
    let mut role_line = role.trim().to_string();
    if let Some(l) = location.map(str::trim).filter(|l| !l.is_empty()) {
        role_line.push_str(&format!(" ({l})"));
    }
    if let Some(c) = client.map(str::trim).filter(|c| !c.is_empty() && !confidential) {
        role_line.push_str(&format!(" at {c}"));
    }
    let why_line = if why.is_empty() { String::new() } else { format!(" It looks like a fit because: {why}.") };
    format!(
        "{greeting}\n\nA new role just came in that matches your profile: {role_line}.{why_line}\n\nIf you're interested, reply to this email and we'll take it forward. Reply STOP to opt out of these updates.\n\nBest regards,\n{sign_off}"
    )
}

/// A new requirement was matched: email each watching candidate who fits. Returns how many sends were scheduled.
pub async fn notify_matches(state: &AppState, ws: &str, requirement_id: &str) -> Result<usize, JobError> {
    if !packs::enabled(&state.db, ws, packs::STAFFING).await? {
        return Ok(0);
    }
    let req: Option<(String, Option<String>, Option<String>, i64)> = sqlx::query_as(
        "SELECT role_title, location, client, confidential FROM requirements WHERE id = ?1 AND workspace_id = ?2 AND status = 'open'",
    )
    .bind(requirement_id)
    .bind(ws)
    .fetch_optional(&state.db)
    .await?;
    let Some((role, location, client, confidential)) = req else { return Ok(0) };
    let now = now_ms();
    let targets: Vec<Target> = sqlx::query_as(
        "SELECT w.id AS watch_id, w.message_id, c.name, w.last_notified_at AS previous, m.reasons_json
         FROM matches m JOIN candidate_watch w ON w.candidate_id = m.candidate_id AND w.workspace_id = m.workspace_id
         JOIN candidates c ON c.id = m.candidate_id
         WHERE m.requirement_id = ?1 AND m.workspace_id = ?2 AND m.score >= ?3 AND m.status != 'dismissed'
           AND w.opted_out = 0 AND (w.last_notified_at IS NULL OR w.last_notified_at < ?4)
         ORDER BY m.score DESC LIMIT 20",
    )
    .bind(requirement_id)
    .bind(ws)
    .bind(i64::from(STRONG_MATCH_SCORE))
    .bind(now - WEEK_MS)
    .fetch_all(&state.db)
    .await?;
    if targets.is_empty() {
        return Ok(0);
    }
    let settings = crate::reply_settings::load(&state.db, &state.config, ws).await?;
    let sign_off = match settings.signature.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => s.to_string(),
        None => packs::default_sign_off(&state.db, ws).await?,
    };
    let mut scheduled = 0;
    for t in targets {
        let body = body_for(&t.name, &role, location.as_deref(), client.as_deref(), confidential == 1, &reasons(&t.reasons_json), &sign_off);
        let draft_id = new_id("drf");
        let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
        let claimed = sqlx::query(
            "UPDATE candidate_watch SET last_notified_at = ?1 WHERE id = ?2 AND opted_out = 0 AND (last_notified_at IS NULL OR last_notified_at < ?3)",
        )
        .bind(now)
        .bind(&t.watch_id)
        .bind(now - WEEK_MS)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        let inserted = claimed
            && sqlx::query(
                "INSERT OR IGNORE INTO drafts (id, workspace_id, message_id, body_text, original_body_text, status, model_id, origin_key,
                                               memory_ids_json, unanswered_json, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4, 'suggested', 'watch-template', ?5, '[]', '[]', ?6, ?6)",
            )
            .bind(&draft_id)
            .bind(ws)
            .bind(&t.message_id)
            .bind(&body)
            .bind(format!("watch:{}:{requirement_id}", t.watch_id))
            .bind(now)
            .execute(&mut *tx)
            .await?
            .rows_affected()
                == 1;
        if !inserted {
            tx.rollback().await?;
            continue;
        }
        audit::record(
            &mut tx,
            Entry {
                workspace_id: ws,
                actor: Actor::System,
                action: "candidate_watch.notified",
                entity_type: "candidate_watch",
                entity_id: &t.watch_id,
                message_id: Some(&t.message_id),
                metadata: json!({ "requirement_id": requirement_id, "draft_id": draft_id, "client_hidden": confidential == 1 }),
            },
        )
        .await?;
        tx.commit().await?;
        match replies::send_draft_held(state, ws, &draft_id, Actor::System, state.config.agent_auto_send_delay_secs).await {
            Ok(_) => scheduled += 1,
            Err(e) => {
                tracing::warn!(watch = %t.watch_id, error = %e, "candidate watch email was not scheduled");
                sqlx::query("UPDATE candidate_watch SET last_notified_at = ?1 WHERE id = ?2").bind(t.previous).bind(&t.watch_id).execute(&state.db).await?;
            }
        }
    }
    Ok(scheduled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_is_recognised_only_as_the_reply_itself() {
        for yes in ["STOP", "stop.", "Stop\n\nOn Mon, Priya wrote:", "unsubscribe", "Stop emails please"] {
            assert!(is_stop(yes), "{yes}");
        }
        for no in ["Please don't stop the process, I'm interested", "I'd like the role", ""] {
            assert!(!is_stop(no), "{no}");
        }
    }

    #[test]
    fn confidential_requirements_hide_the_client() {
        let open = body_for("Ananth Rao", "React Native Developer", Some("Pune"), Some("Capgemini"), false, "skills match", "HR");
        assert!(open.contains("at Capgemini") && open.contains("Hello Ananth,") && open.contains("Reply STOP"), "{open}");
        let hidden = body_for("Ananth Rao", "React Native Developer", Some("Pune"), Some("Capgemini"), true, "", "HR");
        assert!(!hidden.contains("Capgemini"), "{hidden}");
    }
}
