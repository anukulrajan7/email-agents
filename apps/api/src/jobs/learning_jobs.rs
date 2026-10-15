//! Learning jobs: `learn_from_message` (after classification) and `learn_from_signal` (a person finalised a
//! reply, or declined an agent's request with a note). One model call per finalised reply returns both style
//! lessons (stored as preferences) and durable facts (stored as memories).

use serde_json::Value;

use super::handlers::JobError;
use crate::{
    ai::{
        LearnInput, LearnOutput, LearnTask,
        memory_extractor::{LearnDirection, MemoryExtractInput},
    },
    platform::config::UnderstandMode,
    http::AppState,
    learning::{
        edit_distance::{self, TINY_EDIT},
        guard, hooks, preferences::{self, Outcome, Target}, retrieve, scope,
    },
    learning::memory_learning::{self, AUTO_MEMORY_CAP},
};

const MIN_TEXT_CHARS: usize = 20;

fn str_field(payload: &Value, key: &str) -> Result<String, JobError> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| JobError::Permanent(format!("payload missing {key}")))
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    workspace_id: String,
    thread_id: String,
    subject: String,
    sender_address: String,
    body_text: String,
    recipients_json: String,
    source_message_id: String,
    received_at: i64,
    category: Option<String>,
    needs_reply: Option<bool>,
    display_name: Option<String>,
}

/// What the sender wrote this time plus their sign-off (role and company are usually there).
fn incoming_text(subject: &str, body: &str) -> String {
    let split = crate::ai::body::split(subject, body);
    if split.signature.is_empty() { split.new_part } else { format!("{}\n\n{}", split.new_part, split.signature) }
}

pub async fn learn_from_message(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let message_id = str_field(payload, "message_id")?;
    let Some(m): Option<MessageRow> = sqlx::query_as(
        "SELECT m.workspace_id, m.thread_id, m.subject, m.sender_address, m.body_text, m.recipients_json,
                m.source_message_id, m.received_at, c.category,
                COALESCE(i.needs_reply, c.requires_reply) AS needs_reply, ct.display_name
         FROM email_messages m
         LEFT JOIN classifications c ON c.message_id = m.id
         LEFT JOIN message_insights i ON i.message_id = m.id
         LEFT JOIN contacts ct ON ct.workspace_id = m.workspace_id AND ct.email = m.sender_address
         WHERE m.id = ?1",
    )
    .bind(&message_id)
    .fetch_optional(&state.db)
    .await?
    else {
        return Ok(());
    };
    if m.body_text.trim().chars().count() < MIN_TEXT_CHARS {
        return Ok(());
    }
    let team = memory_learning::team_addresses(&state.db, &m.workspace_id).await?;
    let outbound = team.contains(&m.sender_address.to_lowercase());

    let input = if outbound {
        // Mail the team sent from the mailbox. Skip it if it is a reply we already learned from.
        let learned_as_draft: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM drafts WHERE workspace_id = ?1 AND status = 'sent' AND provider_message_id = ?2)",
        )
        .bind(&m.workspace_id)
        .bind(&m.source_message_id)
        .fetch_one(&state.db)
        .await?;
        if learned_as_draft {
            return Ok(());
        }
        let recipients: Vec<String> = serde_json::from_str(&m.recipients_json).unwrap_or_default();
        let counterpart = recipients.into_iter().find(|r| !team.contains(&r.to_lowercase()));
        let context: Option<String> = sqlx::query_scalar(
            "SELECT body_text FROM email_messages WHERE thread_id = ?1 AND received_at < ?2 AND id != ?3
             ORDER BY received_at DESC LIMIT 1",
        )
        .bind(&m.thread_id)
        .bind(m.received_at)
        .bind(&message_id)
        .fetch_optional(&state.db)
        .await?;
        let text = crate::ai::body::model_body(&m.subject, &m.body_text);
        let context = context.map(|c| crate::ai::body::model_body(&m.subject, &c));
        MemoryExtractInput {
            direction: LearnDirection::Reply,
            subject: m.subject,
            counterpart,
            counterpart_name: None,
            label: m.category,
            text,
            context,
        }
    } else {
        // Newsletters/notifications: labelled `other` with nothing to answer, or an automated sender.
        let is_noise = m.category.as_deref() == Some(crate::platform::domain::OTHER_LABEL) && m.needs_reply != Some(true);
        if is_noise || memory_learning::is_automated_sender(&m.sender_address) {
            return Ok(());
        }
        let text = incoming_text(&m.subject, &m.body_text);
        MemoryExtractInput {
            direction: LearnDirection::Incoming,
            subject: m.subject,
            counterpart: Some(m.sender_address.clone()),
            counterpart_name: m.display_name,
            label: m.category,
            text,
            context: None,
        }
    };
    // With understanding v2 the facts came out of the one reading already: no second call on the same email.
    let v2_facts = match (input.direction, state.config.understand_mode) {
        (LearnDirection::Incoming, UnderstandMode::On) => crate::ai::understand::stored(&state.db, &message_id).await?.map(|u| u.facts),
        _ => None,
    };
    let facts = match v2_facts {
        Some(facts) => facts,
        None => state.memory_extractor.extract(&input).await?,
    };
    let learned = memory_learning::sanitize(input.direction, input.counterpart.as_deref(), facts);
    let report =
        memory_learning::store(&state.db, &m.workspace_id, &message_id, &message_id, input.direction, learned, AUTO_MEMORY_CAP)
            .await?;
    tracing::info!(%message_id, created = report.created, merged = report.merged, "learned from message");
    Ok(())
}

/// Queues learning for an approved or sent draft (once per distinct final text).
pub async fn enqueue_reply_learning(pool: &sqlx::SqlitePool, draft_id: &str) -> sqlx::Result<()> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT workspace_id, body_text FROM drafts WHERE id = ?1").bind(draft_id).fetch_optional(pool).await?;
    let Some((ws, body)) = row else { return Ok(()) };
    let mut conn = pool.acquire().await?;
    hooks::enqueue_learning(&mut conn, &ws, draft_id, &body).await
}

/// Dispatches by what taught us: a finalised reply (default, and the payload of older queued jobs) or a decline note.
pub async fn learn_from_signal(state: &AppState, payload: &Value) -> Result<(), JobError> {
    match payload.get("kind").and_then(Value::as_str) {
        Some("decline_note") => learn_from_note(state, payload).await,
        _ => learn_from_reply(state, payload).await,
    }
}

#[derive(sqlx::FromRow)]
struct ReplyRow {
    workspace_id: String,
    message_id: String,
    agent_id: Option<String>,
    original: Option<String>,
    reply: String,
    status: String,
    edit_distance: Option<f64>,
    subject: String,
    sender: String,
    email: String,
    category: Option<String>,
}

pub async fn learn_from_reply(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let draft_id = str_field(payload, "draft_id")?;
    let row: Option<ReplyRow> = sqlx::query_as(
        "SELECT d.workspace_id, d.message_id, d.agent_id, d.original_body_text AS original, d.body_text AS reply, d.status, d.edit_distance,
                m.subject, m.sender_address AS sender, m.body_text AS email, c.category
         FROM drafts d
         JOIN email_messages m ON m.id = d.message_id
         LEFT JOIN classifications c ON c.message_id = m.id
         WHERE d.id = ?1",
    )
    .bind(&draft_id)
    .fetch_optional(&state.db)
    .await?;
    let Some(r) = row else { return Ok(()) };
    if r.status == "suggested" || r.reply.trim().chars().count() < MIN_TEXT_CHARS {
        return Ok(());
    }
    let agent = {
        let mut conn = state.db.acquire().await?;
        scope::resolve(&mut conn, &r.workspace_id, r.agent_id.as_deref(), Some(&r.message_id)).await?
    };
    if !agent.learn_from_edits {
        return Ok(());
    }
    let suggested = r.original.clone().unwrap_or_else(|| r.reply.clone());
    let distance = r.edit_distance.unwrap_or_else(|| edit_distance::normalised(&suggested, &r.reply));
    if distance < TINY_EDIT && !edit_distance::adds_specifics(&suggested, &r.reply, &r.email) {
        tracing::debug!(%draft_id, "reply taught nothing new: skipped");
        return Ok(());
    }
    if !crate::ai::ledger::within_budget(&state.db, state.config.ai_daily_budget_usd, &r.workspace_id).await {
        tracing::debug!(%draft_id, "AI budget reached: learning skipped");
        return Ok(());
    }
    let input = LearnInput {
        task: LearnTask::ApprovedReply { suggested, approved: r.reply.clone() },
        subject: r.subject.clone(),
        counterpart: Some(r.sender.clone()),
        label: r.category.clone(),
        email: crate::ai::body::model_body(&r.subject, &r.email),
    };
    let output = match &state.learner {
        Some(learner) => learner.learn(&input).await?,
        None => LearnOutput {
            style: vec![],
            facts: state
                .memory_extractor
                .extract(&MemoryExtractInput {
                    direction: LearnDirection::Reply,
                    subject: r.subject.clone(),
                    counterpart: Some(r.sender.clone()),
                    counterpart_name: None,
                    label: r.category.clone(),
                    text: r.reply.clone(),
                    context: Some(crate::ai::body::model_body(&r.subject, &r.email)),
                })
                .await?,
        },
    };
    let (suggested_prefs, facts_stored) = apply(state, &r, &agent, output, &draft_id).await?;
    tracing::info!(%draft_id, edit_distance = distance, suggested_prefs, facts_stored, "learned from reply");
    Ok(())
}

/// Stores one learner output: style lessons as suggested preferences, facts as memories. Code filters both.
async fn apply(state: &AppState, r: &ReplyRow, agent: &scope::AgentScope, output: LearnOutput, draft_id: &str) -> Result<(usize, usize), JobError> {
    let target = Target { ws: &r.workspace_id, agent_id: agent.agent_id.as_deref(), user_id: agent.owner_user_id.as_deref() };
    let mut created = Vec::new();
    let mut touched = 0;
    let mut conn = state.db.acquire().await?;
    for (n, lesson) in output.style.iter().enumerate() {
        let evidence = if n == 0 { draft_id.to_string() } else { format!("{draft_id}#{}", n + 1) };
        match preferences::suggest(&mut conn, &target, &lesson.category, &lesson.content, "edit", &evidence).await? {
            Outcome::Created(id) => {
                touched += 1;
                created.push((id, lesson.content.clone()));
            }
            Outcome::Merged(_) => touched += 1,
            Outcome::Tombstoned | Outcome::Rejected(_) => {}
        }
    }
    for (id, content) in created {
        if let Some(blob) = retrieve::embed_one(state.embedder.as_ref(), &content).await {
            preferences::set_embedding(&mut conn, &id, &blob).await?;
        }
    }
    drop(conn);
    let facts = output
        .facts
        .into_iter()
        .filter(|f| !matches!(f.category.as_str(), "preference" | "style") && guard::fact_ok(&f.content))
        .collect();
    let learned = memory_learning::sanitize(LearnDirection::Reply, Some(&r.sender), facts);
    let stored = learned.len();
    memory_learning::store_for(&state.db, &r.workspace_id, agent.fact_agent_id(), &r.message_id, draft_id, LearnDirection::Reply, learned, AUTO_MEMORY_CAP)
        .await?;
    Ok((touched, stored))
}

/// A decline note becomes `process` preferences: suggested only, never active until a person accepts them.
async fn learn_from_note(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let signal_id = str_field(payload, "signal_id")?;
    type NoteRow = (String, Option<String>, Option<String>, Option<String>);
    let row: Option<NoteRow> = sqlx::query_as(
        "SELECT workspace_id, agent_id, note, approval_id FROM feedback_events WHERE id = ?1 AND kind = 'approval_declined'",
    )
    .bind(&signal_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((ws, agent_id, Some(note), approval_id)) = row else { return Ok(()) };
    let Some(learner) = state.learner.clone() else { return Ok(()) };
    if !crate::ai::ledger::within_budget(&state.db, state.config.ai_daily_budget_usd, &ws).await {
        return Ok(());
    }
    let proposed: String = match &approval_id {
        Some(a) => sqlx::query_scalar("SELECT summary FROM agent_approvals WHERE id = ?1").bind(a).fetch_optional(&state.db).await?.unwrap_or_default(),
        None => String::new(),
    };
    let output = learner
        .learn(&LearnInput { task: LearnTask::DeclineNote { proposed, note }, subject: String::new(), counterpart: None, label: None, email: String::new() })
        .await?;
    let target = Target { ws: &ws, agent_id: agent_id.as_deref(), user_id: None };
    let mut conn = state.db.acquire().await?;
    let evidence = approval_id.unwrap_or(signal_id);
    for (n, lesson) in output.style.iter().enumerate() {
        let evidence = if n == 0 { evidence.clone() } else { format!("{evidence}#{}", n + 1) };
        preferences::suggest(&mut conn, &target, guard::PROCESS, &lesson.content, "decline_note", &evidence).await?;
    }
    Ok(())
}
