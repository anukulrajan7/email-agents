//! Candidate watch: a job seeker told "we'll send you matching roles" is remembered. When a new requirement
//! matches them (strong score), they get one email through the normal send path (hold + Stop). It follows the
//! same standing rules as an agent's own send: the lead agent must be on with send "auto" and level 2 or more,
//! the mailbox owner must allow routine replies, the conversation must pass the injection scan, the sensitive
//! floor and a workspace-wide daily cap apply, and the draft is attributed to the lead (no agent, no send).
//! At most one email per candidate per week, STOP opts out (checked again when the send comes due), a
//! confidential requirement never names its client, and every step is audited. Staffing pack only.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};
use sqlx::SqliteConnection;

use crate::{
    platform::audit::{self, Actor, Entry},
    platform::db::{new_id, now_ms},
    http::AppState,
    jobs::{self, handlers::JobError, kind},
    hr::matching::STRONG_MATCH_SCORE,
    platform::packs, replies,
};

pub const WEEK_MS: i64 = 7 * 86_400_000;
const DAY_MS: i64 = 86_400_000;
/// Watch emails per workspace per UTC day.
pub const WATCH_DAILY_CAP: i64 = 20;
/// Candidates handled per run; the rest follow in a later run.
const BATCH: i64 = 20;
pub const ORIGIN_PREFIX: &str = "watch:";
const FREE_MAIL: &[&str] = &["gmail.com", "googlemail.com", "yahoo.com", "outlook.com", "hotmail.com", "icloud.com", "proton.me", "protonmail.com"];

/// Lower-case address with plus-tags removed and, for Gmail, dots removed: the same person's aliases match.
pub fn canonical(address: &str) -> String {
    let a = address.trim();
    let inner = a.rsplit_once('<').and_then(|(_, r)| r.strip_suffix('>')).unwrap_or(a).trim().to_lowercase();
    let Some((local, domain)) = inner.split_once('@') else { return inner };
    let local = local.split('+').next().unwrap_or(local);
    let domain = if domain == "googlemail.com" { "gmail.com" } else { domain };
    let local = if domain == "gmail.com" { local.replace('.', "") } else { local.to_string() };
    format!("{local}@{domain}")
}

static STOP_WORDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(stop|unsubscribe|opt[- ]?out|remove me|no more emails?)\b").expect("valid regex"));
static QUOTE_START: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(>|on .{5,120} wrote:?$|-{2,}\s*(original|forwarded)|from:\s)").expect("valid regex"));

/// The reply's own first lines (before any quoted text) ask to stop. Deliberately broad: it fails toward opting out.
pub fn is_stop(body: &str) -> bool {
    let own: Vec<&str> = body.lines().map(str::trim).take_while(|l| !QUOTE_START.is_match(l)).filter(|l| !l.is_empty()).take(4).collect();
    STOP_WORDS.is_match(&own.join("\n"))
}

/// Remembers a sender who replied STOP, under any alias of the same candidate. Called for every inbound message.
pub async fn opt_out_on_stop(conn: &mut SqliteConnection, ws: &str, sender: &str, body: &str) -> sqlx::Result<()> {
    if !is_stop(body) {
        return Ok(());
    }
    let email = canonical(sender);
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT w.id FROM candidate_watch w JOIN candidates c ON c.id = w.candidate_id
         WHERE w.workspace_id = ?1 AND w.opted_out = 0 AND (w.email = ?2 OR lower(c.email) = lower(?3) OR lower(c.email) = ?2)",
    )
    .bind(ws)
    .bind(&email)
    .bind(sender.trim())
    .fetch_all(&mut *conn)
    .await?;
    for id in ids {
        sqlx::query("UPDATE candidate_watch SET opted_out = 1 WHERE id = ?1").bind(&id).execute(&mut *conn).await?;
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

/// Queues the enrolment check for a sent reply that made the promise. The extraction and the rules run in a job.
pub async fn queue_enrol(conn: &mut SqliteConnection, ws: &str, message_id: &str, draft_id: &str) -> sqlx::Result<()> {
    jobs::enqueue(conn, kind::ENROL_CANDIDATE_WATCH, &format!("watch_enrol:{draft_id}"), Some(ws), &json!({ "workspace_id": ws, "message_id": message_id, "draft_id": draft_id })).await?;
    Ok(())
}

fn made_the_promise(body: &str) -> bool {
    let squash = |t: &str| t.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    squash(body).contains(&squash(crate::replies::gap_resolver::NONE_SENTENCE))
}

fn is_free_mail(domain: &str) -> bool {
    FREE_MAIL.contains(&domain)
}

/// Starts watching for this job seeker. Only a candidate who wrote about themselves: the message is a job-seeker
/// application, not forwarded, the extracted candidate's email is the sender's, the sender is neither a client
/// nor a vendor with several CVs, and the sent reply made the full promise. No candidate rows are created here.
pub async fn enrol_job(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let field = |k: &str| payload.get(k).and_then(Value::as_str).map(str::to_string);
    let (Some(ws), Some(message_id), Some(draft_id)) = (field("workspace_id"), field("message_id"), field("draft_id")) else {
        return Err(JobError::Permanent("payload missing workspace_id/message_id/draft_id".into()));
    };
    let skip = |why: &str| {
        tracing::info!(draft_id, why, "candidate watch not started");
        Ok(())
    };
    if !packs::enabled(&state.db, &ws, packs::STAFFING).await? {
        return skip("staffing pack is off");
    }
    let body: Option<String> = sqlx::query_scalar("SELECT body_text FROM drafts WHERE id = ?1 AND workspace_id = ?2 AND status = 'sent'").bind(&draft_id).bind(&ws).fetch_optional(&state.db).await?;
    if !body.is_some_and(|b| made_the_promise(&b)) {
        return skip("the sent reply did not make the full promise");
    }
    let msg: Option<(String, Option<String>, String, String)> = sqlx::query_as(
        "SELECT m.sender_address, m.original_sender_address, m.subject, COALESCE(c.category, '') FROM email_messages m
         LEFT JOIN classifications c ON c.message_id = m.id WHERE m.id = ?1 AND m.workspace_id = ?2 AND m.direction = 'inbound'",
    )
    .bind(&message_id)
    .bind(&ws)
    .fetch_optional(&state.db)
    .await?;
    let Some((sender, original, subject, label)) = msg else { return skip("no inbound message") };
    if !matches!(label.as_str(), "candidate_referral" | "candidate") {
        return skip("not labelled as a job seeker");
    }
    let sender_key = canonical(&sender);
    let subject_lc = subject.trim().to_lowercase();
    if original.as_deref().is_some_and(|o| canonical(o) != sender_key) || subject_lc.starts_with("fwd:") || subject_lc.starts_with("fw:") {
        return skip("forwarded mail");
    }
    let candidate: Option<(String, Option<String>)> = sqlx::query_as("SELECT id, email FROM candidates WHERE message_id = ?1 AND workspace_id = ?2").bind(&message_id).bind(&ws).fetch_optional(&state.db).await?;
    let Some((candidate_id, candidate_email)) = candidate else { return skip("no candidate record") };
    if candidate_email.as_deref().map(canonical).as_deref() != Some(sender_key.as_str()) {
        return skip("the candidate's email is not the sender's");
    }
    // A sender with several different CVs is a vendor; a requirement's sender (or their company) is a client.
    let others: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT lower(c.name) || '|' || lower(COALESCE(c.email, ''))) FROM candidates c JOIN email_messages m ON m.id = c.message_id
         WHERE c.workspace_id = ?1 AND lower(m.sender_address) = lower(?2)",
    )
    .bind(&ws)
    .bind(sender.trim())
    .fetch_one(&state.db)
    .await?;
    if others > 1 {
        return skip("the sender sent several CVs");
    }
    let domain = sender_key.split('@').nth(1).unwrap_or_default().to_string();
    let client: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM requirements r JOIN email_messages m ON m.id = r.message_id
                        WHERE r.workspace_id = ?1 AND (lower(m.sender_address) = lower(?2) OR (?3 != '' AND lower(m.sender_address) LIKE '%@' || ?3)))",
    )
    .bind(&ws)
    .bind(sender.trim())
    .bind(if is_free_mail(&domain) { "" } else { domain.as_str() })
    .fetch_one(&state.db)
    .await?;
    if client {
        return skip("the sender is a client");
    }
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let id: String = sqlx::query_scalar(
        "INSERT INTO candidate_watch (id, workspace_id, candidate_id, email, message_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(workspace_id, email) DO UPDATE SET candidate_id = excluded.candidate_id, message_id = excluded.message_id
         RETURNING id",
    )
    .bind(new_id("cwt"))
    .bind(&ws)
    .bind(&candidate_id)
    .bind(&sender_key)
    .bind(&message_id)
    .bind(now_ms())
    .fetch_one(&mut *tx)
    .await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &ws,
            actor: Actor::System,
            action: "candidate_watch.enrolled",
            entity_type: "candidate_watch",
            entity_id: &id,
            message_id: Some(&message_id),
            metadata: json!({ "candidate_id": candidate_id }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Plain words only, at most `max` characters: no links, addresses or odd characters from outside text.
pub fn plain(text: &str, max: usize) -> String {
    let kept: Vec<String> = text
        .split_whitespace()
        .filter(|t| !t.contains('@') && !t.contains("://") && !t.contains('<') && !t.contains('>') && !t.to_lowercase().starts_with("www."))
        .map(|t| t.chars().filter(|c| c.is_alphanumeric() || matches!(c, '+' | '#' | '.' | '/' | '&' | '-' | ',' | '(' | ')')).collect::<String>())
        .filter(|t| !t.is_empty())
        .collect();
    let mut out = String::new();
    for word in kept {
        let next = if out.is_empty() { word.clone() } else { format!("{out} {word}") };
        if next.chars().count() > max {
            break;
        }
        out = next;
    }
    out
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

/// The email text. `client` is passed only when the requirement is from a known contact and not confidential.
pub fn body_for(name: &str, role: &str, location: Option<&str>, client: Option<&str>, why: &str, sign_off: &str) -> String {
    let first = name.split_whitespace().next().map(|n| plain(n, 30)).filter(|n| !n.is_empty() && !matches!(n.to_lowercase().as_str(), "unnamed" | "unknown" | "candidate"));
    let greeting = first.map_or("Hello,".to_string(), |n| format!("Hello {n},"));
    let mut role_line = plain(role, 60);
    if role_line.is_empty() {
        role_line = "a new role".into();
    }
    if let Some(l) = location.map(|l| plain(l, 40)).filter(|l| !l.is_empty()) {
        role_line.push_str(&format!(" ({l})"));
    }
    if let Some(c) = client.map(|c| plain(c, 40)).filter(|c| !c.is_empty()) {
        role_line.push_str(&format!(" at {c}"));
    }
    let why = plain(why, 120);
    let why_line = if why.is_empty() { String::new() } else { format!(" It looks like a fit because: {why}.") };
    format!(
        "{greeting}\n\nA new role just came in that matches your profile: {role_line}.{why_line}\n\nIf you're interested, reply to this email and we'll take it forward. Reply STOP to opt out of these updates.\n\nBest regards,\n{sign_off}"
    )
}

/// A requirement from a known contact (we exchanged mail before, in another conversation) or one a person confirmed.
async fn known_source(state: &AppState, ws: &str, requirement_id: &str) -> sqlx::Result<(bool, bool)> {
    let row: Option<(String, String, String, i64, Option<i64>)> = sqlx::query_as(
        "SELECT lower(m.sender_address), m.thread_id, m.id, m.received_at, r.confirmed_at FROM requirements r JOIN email_messages m ON m.id = r.message_id
         WHERE r.id = ?1 AND r.workspace_id = ?2",
    )
    .bind(requirement_id)
    .bind(ws)
    .fetch_optional(&state.db)
    .await?;
    let Some((sender, thread, _msg, received, confirmed)) = row else { return Ok((false, false)) };
    let known: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM drafts d JOIN email_messages m ON m.id = d.message_id
                        WHERE d.workspace_id = ?1 AND d.status = 'sent' AND lower(m.sender_address) = ?2 AND m.thread_id != ?3 AND m.received_at < ?4)
             OR EXISTS (SELECT 1 FROM email_messages o, json_each(o.recipients_json) r
                        WHERE o.workspace_id = ?1 AND o.direction = 'outbound' AND o.thread_id != ?3 AND o.received_at < ?4 AND instr(lower(r.value), ?2) > 0)",
    )
    .bind(ws)
    .bind(&sender)
    .bind(&thread)
    .bind(received)
    .fetch_one(&state.db)
    .await?;
    Ok((known, confirmed.is_some()))
}

pub async fn sent_today(state: &AppState, ws: &str) -> sqlx::Result<i64> {
    let since = now_ms() / DAY_MS * DAY_MS;
    sqlx::query_scalar("SELECT COUNT(*) FROM audit_events WHERE workspace_id = ?1 AND action = 'candidate_watch.notified' AND created_at >= ?2")
        .bind(ws)
        .bind(since)
        .fetch_one(&state.db)
        .await
}

/// Why this watch draft must not be sent right now, if it must. Everything an agent's own send is held to,
/// plus the sensitive floor and the daily cap. Used when scheduling and again when the send comes due.
pub async fn blocker(state: &AppState, ws: &str, draft_id: &str, to: &str, check_cap: bool) -> sqlx::Result<Option<String>> {
    let row: Option<(String, Option<String>)> = sqlx::query_as("SELECT body_text, agent_id FROM drafts WHERE id = ?1 AND workspace_id = ?2").bind(draft_id).bind(ws).fetch_optional(&state.db).await?;
    let Some((body, agent_id)) = row else { return Ok(Some("the draft is gone".into())) };
    if agent_id.is_none() {
        return Ok(Some("no agent stands behind this send".into()));
    }
    if crate::agents::autonomy::is_sensitive(&body) {
        return Ok(Some("it mentions money, rates or a decision about someone".into()));
    }
    if crate::agents::autonomy::leaks(&body, &[to.to_string()]) {
        return Ok(Some("it mentions an address or number from nowhere".into()));
    }
    if let Some(why) = crate::agents::autonomy::automated_send_stopped(state, ws, draft_id).await? {
        return Ok(Some(why.into()));
    }
    if check_cap && sent_today(state, ws).await? >= WATCH_DAILY_CAP {
        return Ok(Some("the daily limit for these emails is reached".into()));
    }
    Ok(None)
}

/// For `SEND_DRAFT`: a watch draft whose candidate opted out, or that no agent stands behind any more, must not go.
pub async fn stop_reason(state: &AppState, ws: &str, draft_id: &str) -> sqlx::Result<Option<String>> {
    let row: Option<(Option<String>, String)> = sqlx::query_as("SELECT origin_key, body_text FROM drafts WHERE id = ?1 AND workspace_id = ?2").bind(draft_id).bind(ws).fetch_optional(&state.db).await?;
    let Some((Some(origin), _)) = row else { return Ok(None) };
    let Some(rest) = origin.strip_prefix(ORIGIN_PREFIX) else { return Ok(None) };
    let watch_id = rest.split(':').next().unwrap_or_default();
    let watch: Option<(i64, String)> = sqlx::query_as("SELECT opted_out, email FROM candidate_watch WHERE id = ?1 AND workspace_id = ?2").bind(watch_id).bind(ws).fetch_optional(&state.db).await?;
    match watch {
        None => Ok(Some("the candidate is no longer on watch".into())),
        Some((1, _)) => Ok(Some("the candidate opted out".into())),
        Some((_, email)) => blocker(state, ws, draft_id, &email, false).await,
    }
}

/// Removes a watch draft that will not be sent, and gives the candidate's weekly slot back.
pub async fn discard(state: &AppState, ws: &str, draft_id: &str) -> sqlx::Result<()> {
    let origin: Option<String> = sqlx::query_scalar("SELECT origin_key FROM drafts WHERE id = ?1 AND workspace_id = ?2").bind(draft_id).bind(ws).fetch_optional(&state.db).await?.flatten();
    let deleted = sqlx::query("DELETE FROM drafts WHERE id = ?1 AND workspace_id = ?2 AND status != 'sent' AND send_attempt_id IS NULL").bind(draft_id).bind(ws).execute(&state.db).await?.rows_affected();
    if deleted == 1
        && let Some(watch_id) = origin.as_deref().and_then(|o| o.strip_prefix(ORIGIN_PREFIX)).and_then(|r| r.split(':').next())
    {
        sqlx::query(
            "UPDATE candidate_watch SET last_notified_at =
               (SELECT MAX(d.updated_at) FROM drafts d WHERE d.workspace_id = ?2 AND d.origin_key LIKE 'watch:' || ?1 || ':%' AND d.status = 'sent')
             WHERE id = ?1",
        )
        .bind(watch_id)
        .bind(ws)
        .execute(&state.db)
        .await?;
    }
    Ok(())
}

#[derive(sqlx::FromRow)]
struct Target {
    watch_id: String,
    message_id: String,
    name: String,
    email: String,
    reasons_json: String,
}

/// What one run did; `more` means candidates are still waiting (the limit or the daily cap stopped it).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    pub scheduled: usize,
    pub more: bool,
    pub cap_reached: bool,
}

/// A new requirement was matched: email each watching candidate who fits, within the rules above.
pub async fn notify_matches(state: &AppState, ws: &str, requirement_id: &str) -> Result<Outcome, JobError> {
    let none = Outcome::default();
    if !packs::enabled(&state.db, ws, packs::STAFFING).await? {
        return Ok(none);
    }
    let req: Option<(String, Option<String>, Option<String>, i64)> = sqlx::query_as(
        "SELECT role_title, location, client, confidential FROM requirements WHERE id = ?1 AND workspace_id = ?2 AND status = 'open'",
    )
    .bind(requirement_id)
    .bind(ws)
    .fetch_optional(&state.db)
    .await?;
    let Some((role, location, client, confidential)) = req else { return Ok(none) };
    let (known_client, confirmed) = known_source(state, ws, requirement_id).await?;
    if !known_client && !confirmed {
        tracing::info!(requirement_id, "candidate watch waits: the requirement is not from a known contact and nobody confirmed it");
        return Ok(none);
    }
    let Some(lead) = crate::agents::registry::store::lead(&state.db, ws).await? else { return Ok(none) };
    let now = now_ms();
    let mut targets: Vec<Target> = sqlx::query_as(
        "SELECT w.id AS watch_id, w.message_id, c.name, w.email, m.reasons_json
         FROM matches m JOIN candidate_watch w ON w.candidate_id = m.candidate_id AND w.workspace_id = m.workspace_id
         JOIN candidates c ON c.id = m.candidate_id
         WHERE m.requirement_id = ?1 AND m.workspace_id = ?2 AND m.score >= ?3 AND m.status != 'dismissed'
           AND w.opted_out = 0 AND (w.last_notified_at IS NULL OR w.last_notified_at < ?4)
           AND NOT EXISTS (SELECT 1 FROM drafts d WHERE d.workspace_id = ?2 AND d.origin_key = 'watch:' || w.id || ':' || ?1)
         ORDER BY m.score DESC, w.id LIMIT ?5",
    )
    .bind(requirement_id)
    .bind(ws)
    .bind(i64::from(STRONG_MATCH_SCORE))
    .bind(now - WEEK_MS)
    .bind(BATCH + 1)
    .fetch_all(&state.db)
    .await?;
    let mut outcome = Outcome { more: targets.len() as i64 > BATCH, ..none };
    targets.truncate(BATCH as usize);
    if targets.is_empty() {
        return Ok(outcome);
    }
    let settings = crate::replies::reply_settings::load(&state.db, &state.config, ws).await?;
    let sign_off = match settings.signature.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => s.to_string(),
        None => packs::default_sign_off(&state.db, ws).await?,
    };
    let confidential = confidential == 1;
    let client_name = client.as_deref().map(str::trim).filter(|c| !c.is_empty());
    let title = match (client_name, confidential) {
        (Some(c), true) => Regex::new(&format!("(?i){}", regex::escape(c))).map(|re| re.replace_all(&role, "").to_string()).unwrap_or_else(|_| role.clone()),
        _ => role.clone(),
    };
    let shown_client = if known_client && !confidential { client_name } else { None };
    for (index, t) in targets.iter().enumerate() {
        if sent_today(state, ws).await? >= WATCH_DAILY_CAP {
            outcome.more = true;
            outcome.cap_reached = true;
            break;
        }
        let why = reasons(&t.reasons_json);
        let why = if confidential && client_name.is_some_and(|c| why.to_lowercase().contains(&c.to_lowercase())) { String::new() } else { why };
        let body = body_for(&t.name, &title, location.as_deref(), shown_client, &why, &sign_off);
        let draft_id = new_id("drf");
        let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
        let claimed = sqlx::query("UPDATE candidate_watch SET last_notified_at = ?1 WHERE id = ?2 AND opted_out = 0 AND (last_notified_at IS NULL OR last_notified_at < ?3)")
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
                                               memory_ids_json, unanswered_json, agent_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4, 'suggested', 'watch-template', ?5, '[]', '[]', ?6, ?7, ?7)",
            )
            .bind(&draft_id)
            .bind(ws)
            .bind(&t.message_id)
            .bind(&body)
            .bind(format!("{ORIGIN_PREFIX}{}:{requirement_id}", t.watch_id))
            .bind(&lead.id)
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
                metadata: json!({ "requirement_id": requirement_id, "draft_id": draft_id, "client_hidden": shown_client.is_none(), "agent_id": lead.id }),
            },
        )
        .await?;
        tx.commit().await?;
        if let Some(why) = blocker(state, ws, &draft_id, &t.email, false).await? {
            tracing::info!(watch = %t.watch_id, why, "candidate watch email held back");
            discard(state, ws, &draft_id).await?;
            continue;
        }
        let hold = state.config.agent_auto_send_delay_secs;
        match replies::send_draft_held(state, ws, &draft_id, Actor::System, hold).await {
            Ok(scheduled) => {
                outcome.scheduled += 1;
                if hold > 0
                    && let Err(e) = crate::agents::autonomy::record_hold(state, ws, &draft_id, scheduled.send_at_ms, hold).await
                {
                    tracing::warn!(error = %e, "could not note the watch send hold");
                }
            }
            Err(e) => {
                tracing::warn!(watch = %t.watch_id, error = %e, "candidate watch email was not scheduled");
                // Another reply went out in the same conversation a moment ago: try again in a later run.
                if format!("{e:?}").contains("\"duplicate\"") {
                    outcome.more = true;
                }
                discard(state, ws, &draft_id).await?;
            }
        }
        let _ = index;
    }
    outcome.cap_reached = outcome.cap_reached || (outcome.more && sent_today(state, ws).await? >= WATCH_DAILY_CAP);
    Ok(outcome)
}

/// `NOTIFY_CANDIDATE_WATCH`: a later run for the candidates a first run could not reach.
pub async fn notify_job(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let (Some(ws), Some(requirement_id)) =
        (payload.get("workspace_id").and_then(Value::as_str), payload.get("requirement_id").and_then(Value::as_str))
    else {
        return Err(JobError::Permanent("payload missing workspace_id/requirement_id".into()));
    };
    let outcome = notify_matches(state, ws, requirement_id).await?;
    schedule_more(state, ws, requirement_id, outcome).await?;
    Ok(())
}

/// Queues the next run when candidates are still waiting: in an hour, or after midnight when the daily cap stopped it.
pub async fn schedule_more(state: &AppState, ws: &str, requirement_id: &str, outcome: Outcome) -> Result<(), JobError> {
    if !outcome.more {
        return Ok(());
    }
    let now = now_ms();
    let at = if outcome.cap_reached { (now / DAY_MS + 1) * DAY_MS + 60_000 } else { now + 3_600_000 };
    let mut conn = state.db.acquire().await?;
    jobs::enqueue_at(
        &mut conn,
        kind::NOTIFY_CANDIDATE_WATCH,
        &format!("watch_notify:{requirement_id}:{}", at / 60_000),
        Some(ws),
        &json!({ "workspace_id": ws, "requirement_id": requirement_id }),
        jobs::DEFAULT_MAX_ATTEMPTS,
        at,
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_of_one_address_are_the_same_candidate() {
        assert_eq!(canonical("Ananth <Ananth.Rao+jobs@GoogleMail.com>"), "ananthrao@gmail.com");
        assert_eq!(canonical("a.b@company.com"), "a.b@company.com");
        assert_eq!(canonical("a.b+x@company.com"), "a.b@company.com");
    }

    #[test]
    fn stop_reads_the_first_unquoted_lines_and_fails_toward_opt_out() {
        for yes in ["STOP", "stop.", "Please unsubscribe me", "Stop\n\nOn Mon, Priya wrote:\n> hello", "I want to opt-out", "remove me from this list", "no more emails please", "Please don't stop the process"] {
            assert!(is_stop(yes), "{yes}");
        }
        for no in ["I'd like the role, call me", "", "Thanks!\n\n> If you want to stop replying to these"] {
            assert!(!is_stop(no), "{no}");
        }
    }

    #[test]
    fn only_plain_words_reach_the_mail() {
        assert_eq!(plain("React Native Developer", 60), "React Native Developer");
        assert_eq!(plain("Dev <script>alert(1)</script> http://evil.test x@y.com", 60), "Dev");
        assert!(plain(&"word ".repeat(40), 60).chars().count() <= 60);
        let body = body_for("Ananth Rao", "React Native Developer https://evil.test", Some("Pune"), Some("Capgemini"), "skills match", "HR");
        assert!(body.contains("at Capgemini") && body.contains("Hello Ananth,") && body.contains("Reply STOP") && !body.contains("evil"), "{body}");
        assert!(!body_for("Ananth Rao", "Java Developer", Some("Pune"), None, "", "HR").contains("Capgemini"));
    }
}
