//! Meera's side of the handoff: the requests filed for a message become one reply. `merge` groups them (one reply per
//! recipient), `render` writes template kinds without a model, and the text is written through `live_draft` under the
//! origin `reply:{message_id}`, so there is still one live draft per message. Sending is not here: the reply case's
//! send step goes through the normal send path, at the governing level of the experts who asked.

use serde_json::json;

use crate::{
    agents::{
        handoff::{self, MessageKind, Recipient, Stored, store},
        profiles,
    },
    http::AppState,
    platform::{
        audit::{self, Actor, Entry},
        packs,
    },
    replies::{
        live_draft::{self, Mode, NewDraft, Written},
        reply_settings,
    },
};

pub mod merge;
pub mod render;
pub mod templates;

/// `drafts.model_id` of a reply written from a template: no model took part.
pub const TEMPLATE_MODEL: &str = "reply-template-v1";

/// Meera as the one who writes: her agent row, so the draft is hers.
#[derive(Debug, Clone)]
pub struct Meera {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Composed {
    /// The reply draft was written (or rewritten); `askers` are the experts behind it.
    Written { draft_id: String, line: String, askers: Vec<Asker>, level: i64, kind: MessageKind },
    /// A draft already stands (a person is working on it, or it is the same text).
    Existing { draft_id: String },
    /// The email was already answered or the send is under way: nothing was written.
    Sent,
    /// Nothing to write: no open request, or the words are not all template.
    Nothing(&'static str),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Asker {
    pub agent_id: String,
    pub name: String,
    pub kind: MessageKind,
    pub job: String,
}

/// "Rohan asked Meera to ask Ravi for a corrected sheet" / "Rohan and Ananya asked Meera to reply to Ravi (recorded; correction needed)".
pub fn handoff_line(askers: &[Asker], meera: &str, who: &str) -> String {
    let mut names: Vec<&str> = vec![];
    for a in askers {
        if !names.contains(&a.name.as_str()) {
            names.push(&a.name);
        }
    }
    let joined = match names.as_slice() {
        [] => "An agent".to_string(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    let phrase = |k: MessageKind| match k {
        MessageKind::Ack => format!("tell {who} the timesheet is recorded"),
        MessageKind::Correction => format!("ask {who} for a corrected sheet"),
        MessageKind::Question => format!("ask {who} a question about the sheet"),
        MessageKind::Holding => format!("tell {who} we are looking into it"),
        MessageKind::Answer | MessageKind::Custom => format!("reply to {who}"),
    };
    match askers {
        [one] => format!("{joined} asked {meera} to {}", phrase(one.kind)),
        _ => {
            let parts: Vec<&str> = askers
                .iter()
                .map(|a| match a.kind {
                    MessageKind::Ack => "recorded",
                    MessageKind::Correction => "correction needed",
                    MessageKind::Question => "question",
                    MessageKind::Holding => "being looked into",
                    MessageKind::Answer | MessageKind::Custom => "reply",
                })
                .collect();
            format!("{joined} asked {meera} to reply to {who} ({})", parts.join("; "))
        }
    }
}

/// "Ravi" from "Ravi Kumar"; nothing when the sender has no name we can trust.
fn first_name(sender_name: Option<&str>) -> Option<String> {
    let first = sender_name?.split_whitespace().next()?.trim_matches(|c: char| !c.is_alphabetic() && c != '\'' && c != '-');
    (first.chars().count() >= 2 && first.chars().all(|c| c.is_alphabetic() || c == '\'' || c == '-')).then(|| first.to_string())
}

async fn asker_names(state: &AppState, ws: &str, merged: &merge::Merged) -> sqlx::Result<Vec<Asker>> {
    let ids: Vec<String> = merged.asked.iter().map(|a| a.agent_id.clone()).collect();
    let names = store::names(&state.db, ws, &ids).await?;
    Ok(merged.asked.iter().zip(names).map(|(a, name)| Asker { agent_id: a.agent_id.clone(), name, kind: a.kind, job: a.job.clone() }).collect())
}

/// The sign-off: the profile voice's signature, else the workspace's, else its default ("HR Operations" for staffing).
async fn sign_off(state: &AppState, ws: &str, message_id: &str) -> sqlx::Result<String> {
    let ctx = profiles::context_for_message(&state.db, &state.config, ws, message_id).await?;
    let settings = reply_settings::load(&state.db, &state.config, ws).await?;
    let (_, signature) = profiles::with_draft_style(ctx.draft_style(&state.config), profiles::draft_voice(&state.db, &state.config, ws, message_id, String::new(), settings.signature)).await?;
    match signature {
        Some(s) => Ok(s),
        None => packs::default_sign_off(&state.db, ws).await,
    }
}

/// Writes (or rewrites) the one reply draft of `message_id` from the requests filed for it.
pub async fn compose_message(state: &AppState, ws: &str, message_id: &str, meera: &Meera) -> sqlx::Result<Composed> {
    let requests: Vec<Stored> = store::open_for_message(&state.db, ws, message_id).await?;
    let merged = merge::merge(&requests);
    let Some(m) = merged.into_iter().find(|m| m.to == Recipient::Sender) else { return Ok(Composed::Nothing("no request is waiting")) };
    let sender_name: Option<String> = sqlx::query_scalar("SELECT sender_name FROM email_messages WHERE id = ?1 AND workspace_id = ?2")
        .bind(message_id)
        .bind(ws)
        .fetch_optional(&state.db)
        .await?
        .flatten();
    let first = first_name(sender_name.as_deref());
    let Some(body) = render::render_template(&m, first.as_deref(), &sign_off(state, ws, message_id).await?) else {
        return Ok(Composed::Nothing("the words are not all template"));
    };
    let askers = asker_names(state, ws, &m).await?;
    let line = handoff_line(&askers, &meera.name, first.as_deref().unwrap_or("the sender"));
    let level = m.governing_level(0);
    let requested_by = json!(askers.iter().zip(&m.asked).map(|(a, q)| json!({"agent_id": a.agent_id, "name": a.name, "kind": a.kind, "job": a.job, "level": q.level})).collect::<Vec<_>>());
    // A reply after one that already went out (a receipt, then the acknowledgement) is a second draft with its own origin.
    let sent: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM drafts WHERE message_id = ?1 AND status = 'sent' AND origin_key LIKE 'reply:%'").bind(message_id).fetch_one(&state.db).await?;
    let origin = if sent == 0 { format!("reply:{message_id}") } else { format!("reply:{message_id}:{}", sent + 1) };
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let written = live_draft::write(
        &mut tx,
        Mode::Replace { allow_person_edited: false },
        &NewDraft {
            workspace_id: ws,
            message_id,
            origin_key: &origin,
            body_text: &body,
            model_id: TEMPLATE_MODEL,
            memory_ids_json: "[]".into(),
            unanswered_json: "[]".into(),
            audit: json!({"by": "meera", "template": m.kind, "requested_by": requested_by}),
        },
    )
    .await?;
    let ids: Vec<String> = m.asked.iter().map(|a| a.request_id.clone()).collect();
    let result = match &written {
        Written::Inserted { id, retired } | Written::Replaced { id, retired, .. } => {
            sqlx::query("UPDATE drafts SET agent_id = ?1, requested_by_json = ?2 WHERE id = ?3 AND workspace_id = ?4")
                .bind(&meera.id)
                .bind(requested_by.to_string())
                .bind(id)
                .bind(ws)
                .execute(&mut *tx)
                .await?;
            store::mark_composed(&mut tx, ws, &ids, id).await?;
            let doomed: Vec<String> = retired.iter().map(|r| r.draft_id.clone()).collect();
            store::supersede_drafts(&mut tx, ws, &doomed, message_id).await?;
            audit::record(
                &mut tx,
                Entry {
                    workspace_id: ws,
                    actor: Actor::System,
                    action: "handoff.composed",
                    entity_type: "draft",
                    entity_id: id,
                    message_id: Some(message_id),
                    metadata: json!({"kind": m.kind, "requested_by": requested_by, "governing_level": level, "line": line}),
                },
            )
            .await?;
            Composed::Written { draft_id: id.clone(), line: line.clone(), askers: askers.clone(), level, kind: m.kind }
        }
        Written::Existing(d) => {
            store::mark_composed(&mut tx, ws, &ids, &d.id).await?;
            Composed::Existing { draft_id: d.id.clone() }
        }
        Written::Busy(_) | Written::AlreadySent(_) => Composed::Sent,
    };
    tx.commit().await?;
    live_draft::settle(state, &written).await;
    Ok(result)
}

/// A request changed after its reply was written and before it was sent: merge again into the same draft, and show the
/// new text on the approval that is waiting for it.
pub async fn refresh(state: &AppState, ws: &str, message_id: &str) -> sqlx::Result<()> {
    let case: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT id, agent_id FROM cases WHERE workspace_id = ?1 AND message_id = ?2 AND (job = ?3 OR job LIKE 'reply#%') AND shadow = 0 ORDER BY created_at DESC, rowid DESC LIMIT 1",
    )
        .bind(ws)
        .bind(message_id)
        .bind(handoff::REPLY_JOB)
        .fetch_optional(&state.db)
        .await?;
    let Some((case_id, Some(agent_id))) = case else { return Ok(()) };
    let name: String = sqlx::query_scalar("SELECT name FROM agents WHERE id = ?1 AND workspace_id = ?2").bind(&agent_id).bind(ws).fetch_one(&state.db).await?;
    if let Composed::Written { draft_id, line, .. } = compose_message(state, ws, message_id, &Meera { id: agent_id, name }).await? {
        sqlx::query(
            "UPDATE agent_approvals SET body = (SELECT body_text FROM drafts WHERE id = ?1),
                    input_json = json_set(input_json, '$.handoff.line', ?2) WHERE case_id = ?3 AND workspace_id = ?4 AND status = 'pending'",
        )
        .bind(&draft_id)
        .bind(&line)
        .bind(&case_id)
        .bind(ws)
        .execute(&state.db)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asker(name: &str, kind: MessageKind) -> Asker {
        Asker { agent_id: format!("a-{name}"), name: name.into(), kind, job: "desk:timesheet".into() }
    }

    #[test]
    fn the_approval_line_names_who_asked_and_what_for() {
        assert_eq!(handoff_line(&[asker("Rohan", MessageKind::Correction)], "Meera", "Ravi"), "Rohan asked Meera to ask Ravi for a corrected sheet");
        assert_eq!(handoff_line(&[asker("Rohan", MessageKind::Ack)], "Meera", "Ravi"), "Rohan asked Meera to tell Ravi the timesheet is recorded");
        assert_eq!(
            handoff_line(&[asker("Rohan", MessageKind::Ack), asker("Ananya", MessageKind::Correction)], "Meera", "Ravi"),
            "Rohan and Ananya asked Meera to reply to Ravi (recorded; correction needed)"
        );
    }

    #[test]
    fn the_greeting_uses_a_real_first_name_only() {
        assert_eq!(first_name(Some("Ravi Kumar")).as_deref(), Some("Ravi"));
        assert_eq!(first_name(Some("  priya  ")).as_deref(), Some("priya"));
        assert_eq!(first_name(Some("hr-team@acme.com")), None, "an address is not a name");
        assert_eq!(first_name(Some("")), None);
        assert_eq!(first_name(None), None);
    }
}
