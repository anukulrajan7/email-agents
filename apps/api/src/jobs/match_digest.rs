//! Daily match digest: once per UTC day, at the workspace's alert `digest_hour`, the alert recipients get the new
//! strong matches (still `suggested`, score ≥ 70, created in the last 24 hours, open requirement), grouped by
//! requirement with links. Nothing is sent when there are none. A `match_digests` row claims the day before
//! sending, so a retried job never sends twice. Follows the alert settings' `daily_digest` switch.

use std::collections::HashMap;

use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::{handlers::JobError, kind};
use crate::{
    alerts::{escape_html, load_settings},
    platform::db::now_ms,
    http::AppState,
    jobs,
    hr::matching::STRONG_MATCH_SCORE,
};

const DAY_MS: i64 = 86_400_000;
const HOUR_MS: i64 = 3_600_000;
/// Matches listed in one digest.
const MAX_MATCHES: i64 = 100;
/// Reasons shown per match.
const REASONS_SHOWN: usize = 2;

/// One requirement and its new strong matches.
#[derive(Debug, Clone, PartialEq)]
pub struct DigestGroup {
    pub title: String,
    pub link: String,
    pub matches: Vec<DigestLine>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DigestLine {
    pub name: String,
    pub score: i64,
    pub why: String,
}

#[derive(sqlx::FromRow)]
struct Row {
    requirement_id: String,
    role_title: String,
    client: Option<String>,
    candidate_name: String,
    score: i64,
    reasons_json: String,
}

pub fn matching_link(base: &str, workspace_id: &str, requirement_id: &str) -> String {
    format!("{}/w/{workspace_id}/matching?req={requirement_id}", base.trim_end_matches('/'))
}

fn why(reasons_json: &str) -> String {
    let reasons: Vec<Value> = serde_json::from_str(reasons_json).unwrap_or_default();
    reasons
        .iter()
        .filter(|r| r.get("ok").and_then(Value::as_bool).unwrap_or(false))
        .filter_map(|r| r.get("text").and_then(Value::as_str))
        .take(REASONS_SHOWN)
        .collect::<Vec<_>>()
        .join("; ")
}

async fn new_strong_matches(db: &SqlitePool, base: &str, workspace_id: &str, now: i64) -> sqlx::Result<Vec<DigestGroup>> {
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT m.requirement_id, r.role_title, r.client, c.name AS candidate_name, m.score, m.reasons_json
         FROM matches m JOIN requirements r ON r.id = m.requirement_id JOIN candidates c ON c.id = m.candidate_id
         WHERE m.workspace_id = ?1 AND m.status = 'suggested' AND m.score >= ?2 AND m.created_at > ?3 AND m.created_at <= ?4
           AND r.status = 'open'
         ORDER BY r.created_at DESC, r.id, m.score DESC, c.name
         LIMIT ?5",
    )
    .bind(workspace_id)
    .bind(STRONG_MATCH_SCORE as i64)
    .bind(now - DAY_MS)
    .bind(now)
    .bind(MAX_MATCHES)
    .fetch_all(db)
    .await?;
    let mut groups: Vec<DigestGroup> = vec![];
    let mut index: HashMap<String, usize> = HashMap::new();
    for r in rows {
        let i = *index.entry(r.requirement_id.clone()).or_insert_with(|| {
            groups.push(DigestGroup {
                title: match &r.client {
                    Some(c) if !c.is_empty() => format!("{} ({c})", r.role_title),
                    _ => r.role_title.clone(),
                },
                link: matching_link(base, workspace_id, &r.requirement_id),
                matches: vec![],
            });
            groups.len() - 1
        });
        groups[i].matches.push(DigestLine { name: r.candidate_name, score: r.score, why: why(&r.reasons_json) });
    }
    Ok(groups)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Plain-text + simple HTML digest (same look as the alert emails).
pub fn render_digest(workspace_name: &str, base: &str, workspace_id: &str, groups: &[DigestGroup]) -> (String, String, String) {
    let total: usize = groups.iter().map(|g| g.matches.len()).sum();
    let subject = format!(
        "[Post Room] {} for {}",
        plural(total, "new strong match", "new strong matches"),
        plural(groups.len(), "requirement", "requirements")
    );
    let intro = format!("New strong matches in {workspace_name} (score {STRONG_MATCH_SCORE}+, last 24 hours):");
    let all_link = format!("{}/w/{workspace_id}/matching", base.trim_end_matches('/'));
    let mut text = format!("Hi,\n\n{intro}\n\n");
    for g in groups {
        text.push_str(&format!("{}\n  Open: {}\n", g.title, g.link));
        for m in &g.matches {
            text.push_str(&format!("  - {} ({}): {}\n", m.name, m.score, m.why));
        }
        text.push('\n');
    }
    text.push_str(&format!("--\nPost Room matching. You decide who to shortlist: {all_link}\n"));

    let items: String = groups
        .iter()
        .map(|g| {
            let lines: String = g
                .matches
                .iter()
                .map(|m| {
                    format!(
                        r#"<li style="margin:0 0 6px"><strong>{}</strong> · {}<br><span style="color:#5E5A50">{}</span></li>"#,
                        escape_html(&m.name),
                        m.score,
                        escape_html(&m.why)
                    )
                })
                .collect();
            format!(
                r#"<li style="margin:0 0 16px"><a href="{}" style="color:#34557A;font-weight:600">{}</a><ul style="padding-left:18px;margin:8px 0">{lines}</ul></li>"#,
                escape_html(&g.link),
                escape_html(&g.title)
            )
        })
        .collect();
    let html = format!(
        r#"<div style="font-family:Arial,sans-serif;background:#F3EFE6;padding:32px;color:#1C1B18">
<div style="height:6px;background:repeating-linear-gradient(135deg,#B73A26 0 10px,#F3EFE6 10px 20px,#34557A 20px 30px,#F3EFE6 30px 40px)"></div>
<h1 style="font-family:Georgia,serif;font-weight:600;font-size:22px;margin:24px 0 8px">{}</h1>
<ul style="padding-left:18px;margin:16px 0">{items}</ul>
<p style="font-size:12px;color:#5E5A50"><a href="{}" style="color:#5E5A50">All requirements and matches</a></p>
</div>"#,
        escape_html(&intro),
        escape_html(&all_link)
    );
    (subject, text, html)
}

/// Sends today's digest for one workspace at time `now` (injectable for tests). Returns emails sent; 0 when
/// there is no mailer, the digest is switched off, nothing new qualifies, or today's digest was already claimed.
pub async fn send_match_digest(state: &AppState, workspace_id: &str, now: i64) -> sqlx::Result<usize> {
    let Some(mailer) = state.alert_mailer.clone() else {
        return Ok(0);
    };
    let settings = load_settings(&state.db, workspace_id).await?;
    if !settings.daily_digest || settings.recipients.is_empty() {
        return Ok(0);
    }
    let base = state.config.app_base_url.trim_end_matches('/').to_string();
    let groups = new_strong_matches(&state.db, &base, workspace_id, now).await?;
    if groups.is_empty() {
        return Ok(0);
    }
    let total: usize = groups.iter().map(|g| g.matches.len()).sum();
    let claimed = sqlx::query(
        "INSERT INTO match_digests (workspace_id, day, match_count, recipients_json, sent_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(workspace_id, day) DO NOTHING",
    )
    .bind(workspace_id)
    .bind(now / DAY_MS)
    .bind(total as i64)
    .bind(json!(settings.recipients).to_string())
    .bind(now)
    .execute(&state.db)
    .await?
    .rows_affected()
        == 1;
    if !claimed {
        return Ok(0);
    }
    let ws_name: String = sqlx::query_scalar("SELECT name FROM workspaces WHERE id = ?1")
        .bind(workspace_id)
        .fetch_optional(&state.db)
        .await?
        .unwrap_or_else(|| "your workspace".into());
    let (subject, text, html) = render_digest(&ws_name, &base, workspace_id, &groups);
    let mut sent = 0;
    for to in &settings.recipients {
        let email = crate::platform::mailer::OutgoingEmail { kind: crate::platform::mailer::kind::MATCH_DIGEST, to, subject: &subject, text: &text, html: &html };
        match crate::platform::mailer::deliver(&state.db, mailer.as_ref(), email).await {
            Ok(()) => sent += 1,
            Err(e) => tracing::warn!(error = %e, "match digest email failed"),
        }
    }
    Ok(sent)
}

/// Enqueues each workspace's digest for today, due at its digest hour (UTC). Called from the worker's maintenance
/// tick; the per-day dedupe key makes repeat calls no-ops.
pub async fn schedule(state: &AppState) -> sqlx::Result<()> {
    let now = now_ms();
    let day = now / DAY_MS;
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT w.id, COALESCE(s.digest_hour, 9) FROM workspaces w LEFT JOIN alert_settings s ON s.workspace_id = w.id",
    )
    .fetch_all(&state.db)
    .await?;
    let mut conn = state.db.acquire().await?;
    for (ws, hour) in rows {
        jobs::enqueue_at(
            &mut conn,
            kind::SEND_MATCH_DIGEST,
            &format!("match-digest:{ws}:{day}"),
            Some(&ws),
            &json!({ "workspace_id": ws }),
            3,
            day * DAY_MS + hour * HOUR_MS,
        )
        .await?;
    }
    Ok(())
}

pub async fn digest_job(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let ws = payload
        .get("workspace_id")
        .and_then(Value::as_str)
        .ok_or_else(|| JobError::Permanent("payload missing workspace_id".into()))?;
    let sent = send_match_digest(state, ws, now_ms()).await?;
    if sent > 0 {
        tracing::info!(workspace_id = %ws, emails = sent, "match digest sent");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hr::matching::Reason;

    #[test]
    fn digest_groups_by_requirement_with_links_and_escaping() {
        let groups = vec![
            DigestGroup {
                title: "Java Developer (Capgemini)".into(),
                link: matching_link("https://app/", "ws", "req_1"),
                matches: vec![
                    DigestLine { name: "Rahul <R>".into(), score: 91, why: "Has 3/3 must-have skills: java".into() },
                    DigestLine { name: "Asha".into(), score: 72, why: String::new() },
                ],
            },
            DigestGroup { title: "QA".into(), link: matching_link("https://app", "ws", "req_2"), matches: vec![] },
        ];
        let (subject, text, html) = render_digest("Acme", "https://app", "ws", &groups);
        assert_eq!(subject, "[Post Room] 2 new strong matches for 2 requirements");
        assert!(text.contains("Java Developer (Capgemini)\n  Open: https://app/w/ws/matching?req=req_1\n  - Rahul <R> (91): Has 3/3"));
        assert!(html.contains("Rahul &lt;R&gt;") && html.contains(r#"href="https://app/w/ws/matching?req=req_2""#));
        let one = [DigestGroup { matches: groups[0].matches[..1].to_vec(), ..groups[0].clone() }];
        assert_eq!(render_digest("Acme", "https://app", "ws", &one).0, "[Post Room] 1 new strong match for 1 requirement");
    }

    #[test]
    fn why_keeps_the_first_positive_reasons() {
        let reasons = vec![
            Reason { ok: false, text: "Missing: kafka".into() },
            Reason { ok: true, text: "Has 2/3 must-have skills: java, spring boot".into() },
            Reason { ok: true, text: "6 yrs fits 5–8 yrs".into() },
            Reason { ok: true, text: "Location Pune matches".into() },
        ];
        assert_eq!(why(&serde_json::to_string(&reasons).unwrap()), "Has 2/3 must-have skills: java, spring boot; 6 yrs fits 5–8 yrs");
        assert_eq!(why("not json"), "");
    }
}
