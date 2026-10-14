//! People decide on risky agent actions. Approve applies the action (optionally after editing the
//! draft); decline is recorded for the planner and the same send is never retried automatically.

use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::{
    case::{self, NewStep},
    sentence,
    tools::{self, ToolCtx, ToolError},
};
use crate::{
    audit::{self, Actor, Entry},
    db::{iso, now_ms},
    error::{AppError, AppResult},
    http::AppState,
    learning::{
        edit_distance,
        hooks::{self, Via},
        signals::{self, Kind, Signal},
    },
};

pub const MAX_EDITED_BODY_CHARS: usize = 10_000;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ApprovalRow {
    pub id: String,
    pub case_id: String,
    pub case_title: String,
    pub step_seq: Option<i64>,
    pub action: String,
    pub summary: String,
    pub to_address: Option<String>,
    pub subject: Option<String>,
    pub body: String,
    pub draft_id: Option<String>,
    pub candidates_json: String,
    pub input_json: String,
    pub status: String,
    pub decided_by_email: Option<String>,
    pub decided_at: Option<i64>,
    pub created_at: i64,
    /// The person the agent works for on this case.
    pub agent_user_id: Option<String>,
    /// The agent that asked.
    pub agent_id: Option<String>,
}

const APPROVAL_SELECT: &str = "SELECT a.id, a.case_id, c.title AS case_title, a.step_seq, a.action, a.summary, a.to_address, a.subject,
        a.body, a.draft_id, a.candidates_json, a.input_json, a.status, u.email AS decided_by_email, a.decided_at, a.created_at,
        c.agent_user_id, a.agent_id
 FROM agent_approvals a JOIN cases c ON c.id = a.case_id LEFT JOIN users u ON u.id = a.decided_by";

impl ApprovalRow {
    pub fn to_json(&self) -> Value {
        // A suggested automation: what would be created, for the approver to read.
        let automation = (self.action == "create_automation")
            .then(|| serde_json::from_str::<Value>(&self.input_json).ok())
            .flatten()
            .map(|i| json!({"name": i.get("name"), "conditions": i.get("conditions").cloned().unwrap_or_else(|| json!([])),
                            "actions": i.get("actions").cloned().unwrap_or_else(|| json!([]))}));
        json!({
            "automation": automation,
            "id": self.id, "case_id": self.case_id, "case_title": self.case_title, "step_seq": self.step_seq,
            "action": self.action, "summary": self.summary, "to": self.to_address, "subject": self.subject, "body": self.body,
            "candidates": serde_json::from_str::<Value>(&self.candidates_json).unwrap_or_else(|_| json!([])),
            "status": self.status, "decided_by": self.decided_by_email, "decided_at": self.decided_at.map(iso),
            "created_at": iso(self.created_at),
        })
    }
}

/// Approvals the viewer may see: another member's personal agent's requests are never listed.
pub async fn list(db: &SqlitePool, ws: &str, viewer: &str, status: Option<&str>, case_id: Option<&str>) -> sqlx::Result<Vec<ApprovalRow>> {
    sqlx::query_as(&format!(
        "{APPROVAL_SELECT} WHERE a.workspace_id = ?1 AND (?2 IS NULL OR a.status = ?2) AND (?3 IS NULL OR a.case_id = ?3)
           AND {}
         ORDER BY a.created_at DESC LIMIT 200",
        crate::agent_registry::store::hide_foreign_personal("a", "?4")
    ))
    .bind(ws)
    .bind(status)
    .bind(case_id)
    .bind(viewer)
    .fetch_all(db)
    .await
}

pub async fn get(db: &SqlitePool, ws: &str, viewer: &str, id: &str) -> sqlx::Result<Option<ApprovalRow>> {
    sqlx::query_as(&format!(
        "{APPROVAL_SELECT} WHERE a.id = ?1 AND a.workspace_id = ?2 AND {}",
        crate::agent_registry::store::hide_foreign_personal("a", "?3")
    ))
    .bind(id)
    .bind(ws)
    .bind(viewer)
    .fetch_optional(db)
    .await
}

pub async fn pending_count(db: &SqlitePool, ws: &str, viewer: &str) -> sqlx::Result<i64> {
    sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM agent_approvals a WHERE a.workspace_id = ?1 AND a.status = 'pending' AND {}",
        crate::agent_registry::store::hide_foreign_personal("a", "?2")
    ))
    .bind(ws)
    .bind(viewer)
    .fetch_one(db)
    .await
}

/// Fingerprint of exactly what the person approved: the live outgoing recipient and subject, and the final text.
fn approved_hash(approval: &ApprovalRow, live: Option<&tools::Preview>, body: &str) -> String {
    match live {
        Some(p) => fingerprint(p.to.as_deref(), p.subject.as_deref(), body),
        None => fingerprint(approval.to_address.as_deref(), approval.subject.as_deref(), body),
    }
}

fn fingerprint(to: Option<&str>, subject: Option<&str>, body: &str) -> String {
    use sha2::{Digest, Sha256};
    let material = format!("{}\n{}\n{}", to.unwrap_or(""), subject.unwrap_or(""), body.trim());
    Sha256::digest(material.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// What the tool would send right now (recipient, subject, body), for approvals that send a draft.
async fn live_preview(state: &AppState, case: &case::Case, approval: &ApprovalRow, user_id: &str) -> Option<tools::Preview> {
    approval.draft_id.as_ref()?;
    let tool = tools::registry().get(tools::tool_for_approval(&approval.action))?;
    let args: Value = serde_json::from_str(&approval.input_json).unwrap_or_else(|_| json!({}));
    let ctx = ToolCtx { state, case, turn: case.step_count, actor: Actor::User(user_id), hold_secs: 0 };
    tool.preview(&ctx, &args).await.ok().flatten()
}

/// Puts a decision back to pending so a person can look at the changed reply and decide again.
async fn reopen(state: &AppState, ws: &str, approval_id: &str, user_id: &str, case_id: &str) -> sqlx::Result<()> {
    let mut tx = state.db.begin().await?;
    let reopened = sqlx::query(
        "UPDATE agent_approvals SET status = 'pending', decided_by = NULL, decided_at = NULL, approved_hash = NULL
         WHERE id = ?1 AND workspace_id = ?2 AND status = 'approved'",
    )
    .bind(approval_id)
    .bind(ws)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if reopened {
        audit::record(
            &mut tx,
            Entry {
                workspace_id: ws,
                actor: Actor::User(user_id),
                action: "agent_approval.reopened",
                entity_type: "agent_approval",
                entity_id: approval_id,
                message_id: None,
                metadata: json!({"case_id": case_id, "reason": "the reply changed after it was approved"}),
            },
        )
        .await?;
    }
    tx.commit().await
}

pub struct Decision<'a> {
    pub approve: bool,
    pub edited_body: Option<&'a str>,
    pub note: Option<&'a str>,
}

/// Records a person's decision, applies an approval, and lets the case continue. Audited.
pub async fn decide(state: &AppState, ws: &str, approval_id: &str, user_id: &str, d: Decision<'_>) -> AppResult<ApprovalRow> {
    let approval = get(&state.db, ws, user_id, approval_id).await?.ok_or(AppError::NotFound)?;
    if approval.status != "pending" {
        return Err(AppError::Conflict("this approval was already decided".into()));
    }
    let case = case::get_in(&state.db, ws, &approval.case_id).await?.ok_or(AppError::NotFound)?;
    if case.status != "waiting_approval" {
        return Err(AppError::Conflict("this case is no longer waiting for approval".into()));
    }
    let edited = match (d.approve, d.edited_body.map(str::trim)) {
        (true, Some("")) => return Err(AppError::BadRequest("the edited reply is empty".into())),
        (true, Some(body)) if body.chars().count() > MAX_EDITED_BODY_CHARS => {
            return Err(AppError::BadRequest(format!("the edited reply can be at most {MAX_EDITED_BODY_CHARS} characters")));
        }
        (true, Some(body)) if body != approval.body.trim() => Some(body.to_string()),
        _ => None,
    };
    let note = d.note.map(|n| sentence(n, 300)).filter(|n| !n.is_empty());
    let now = now_ms();
    let live = if d.approve { live_preview(state, &case, &approval, user_id).await } else { None };
    let hash = d.approve.then(|| approved_hash(&approval, live.as_ref(), edited.as_deref().unwrap_or(&approval.body)));

    let mut tx = state.db.begin().await?;
    let decided = sqlx::query(
        "UPDATE agent_approvals SET status = ?1, decided_by = ?2, decided_at = ?3, note = ?4, body = COALESCE(?5, body), approved_hash = ?7
         WHERE id = ?6 AND status = 'pending'",
    )
    .bind(if d.approve { "approved" } else { "declined" })
    .bind(user_id)
    .bind(now)
    .bind(&note)
    .bind(&edited)
    .bind(approval_id)
    .bind(&hash)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !decided {
        return Err(AppError::Conflict("this approval was already decided".into()));
    }
    if let (Some(body), Some(draft_id)) = (&edited, &approval.draft_id) {
        sqlx::query("UPDATE drafts SET body_text = ?1, status = 'edited', updated_at = ?2 WHERE id = ?3 AND workspace_id = ?4 AND status != 'sent'")
            .bind(body)
            .bind(now)
            .bind(draft_id)
            .bind(ws)
            .execute(&mut *tx)
            .await?;
    }
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor: Actor::User(user_id),
            action: if d.approve { "agent_approval.approved" } else { "agent_approval.declined" },
            entity_type: "agent_approval",
            entity_id: approval_id,
            message_id: case.message_id.as_deref(),
            metadata: json!({"case_id": case.id, "action": approval.action, "edited": edited.is_some(), "draft_id": approval.draft_id}),
        },
    )
    .await?;
    if d.approve {
        match &approval.draft_id {
            Some(draft_id) => {
                let via = Via::Approval { approval_id, case_id: &case.id, agent_id: approval.agent_id.as_deref() };
                hooks::draft_finalised(&mut tx, ws, user_id, draft_id, via).await?;
            }
            None => {
                let distance = edited.as_deref().map(|e| edit_distance::normalised(&approval.body, e));
                let kind = if distance.is_some_and(|x| x >= edit_distance::TINY_EDIT) { Kind::ApprovalEdited } else { Kind::ApprovalApproved };
                let signal = Signal::new(ws, kind)
                    .agent(approval.agent_id.as_deref())
                    .actor(user_id)
                    .message(case.message_id.as_deref())
                    .case(&case.id)
                    .approval(approval_id)
                    .distance(distance);
                signals::record(&mut tx, signal).await?;
            }
        }
    } else {
        let declined = hooks::Declined {
            ws,
            user_id,
            approval_id,
            case_id: &case.id,
            agent_id: approval.agent_id.as_deref(),
            message_id: case.message_id.as_deref(),
            note: note.as_deref(),
        };
        hooks::approval_declined(&mut tx, declined).await?;
    }
    tx.commit().await?;

    // Apply outside any transaction (the send is a network call); its own guards still hold.
    let step = if d.approve {
        let tool = tools::registry().get(tools::tool_for_approval(&approval.action)).ok_or_else(|| AppError::Internal(anyhow::anyhow!("unknown approval action")))?;
        let args: Value = serde_json::from_str(&approval.input_json).unwrap_or_else(|_| json!({}));
        let ctx = ToolCtx { state, case: &case, turn: case.step_count, actor: Actor::User(user_id), hold_secs: 0 };
        let now_live = live_preview(state, &case, &approval, user_id).await;
        let unchanged = now_live.as_ref().is_none_or(|p| Some(approved_hash(&approval, Some(p), &p.body)) == hash);
        if !unchanged {
            reopen(state, ws, approval_id, user_id, &case.id).await?;
            return get(&state.db, ws, user_id, approval_id).await?.ok_or(AppError::NotFound);
        }
        let ran = tool.run(&ctx, &args).await;
        match ran {
            Ok(out) => (format!("Approved by a person. {}", out.summary), out.value, "ok", out.state),
            Err(ToolError::Failed(m)) => {
                (format!("Approved, but it could not be sent: {m}"), json!({"error": m}), "error", None)
            }
            Err(ToolError::Db(e)) => return Err(e.into()),
        }
    } else {
        let why = note.as_deref().map(|n| format!(" Note: {n}")).unwrap_or_default();
        (format!("A person declined: {}.{why}", approval.summary), json!({"declined": true, "note": note}), "ok", None)
    };
    let (summary, output, status, patch) = step;

    let mut tx = state.db.begin().await?;
    case::insert_step(
        &mut tx,
        &case,
        NewStep {
            turn: case.step_count,
            kind: if d.approve { "approval" } else { "note" },
            tool: tools::registry().get(tools::tool_for_approval(&approval.action)).map(|t| t.name()),
            summary,
            input: json!({"approval_id": approval_id, "decision": if d.approve { "approve" } else { "decline" }, "edited": edited.is_some()}),
            output,
            status,
            tokens_in: 0,
            tokens_out: 0,
            cost_usd: 0.0,
            model_id: None,
        },
    )
    .await?;
    if let Some(patch) = &patch {
        case::merge_state(&mut tx, ws, &case.id, patch).await?;
    }
    if !d.approve && let Some(draft_id) = &approval.draft_id {
        case::merge_state(&mut tx, ws, &case.id, &json!({"declined_draft_id": draft_id})).await?;
    }
    if case::set_status(&mut tx, ws, &case.id, "running").await? {
        case::enqueue_step(&mut tx, ws, &case.id, case.step_count + 1, now_ms()).await?;
    }
    tx.commit().await?;
    state.jobs_notify.notify_one();
    get(&state.db, ws, user_id, approval_id).await?.ok_or(AppError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approval() -> ApprovalRow {
        ApprovalRow {
            id: "apr_1".into(),
            case_id: "case_1".into(),
            case_title: "t".into(),
            step_seq: None,
            action: "send_email".into(),
            summary: "s".into(),
            to_address: Some("old@client.com".into()),
            subject: Some("Re: Old".into()),
            body: "Hello".into(),
            draft_id: Some("drf_1".into()),
            candidates_json: "[]".into(),
            input_json: "{}".into(),
            status: "pending".into(),
            decided_by_email: None,
            decided_at: None,
            created_at: 0,
            agent_user_id: None,
            agent_id: None,
        }
    }

    fn live(to: &str, subject: &str) -> tools::Preview {
        tools::Preview { to: Some(to.into()), subject: Some(subject.into()), body: "Hello".into(), ..Default::default() }
    }

    #[test]
    fn the_approved_hash_covers_the_live_recipient_subject_and_body() {
        let a = approval();
        let now = live("ravi@client.com", "Re: Status");
        let hash = approved_hash(&a, Some(&now), "Hello");
        assert_eq!(hash, fingerprint(Some("ravi@client.com"), Some("Re: Status"), "Hello"), "the live values, not the request-time snapshot");
        assert_ne!(hash, approved_hash(&a, None, "Hello"));
        assert_ne!(hash, approved_hash(&a, Some(&live("other@client.com", "Re: Status")), "Hello"), "recipient");
        assert_ne!(hash, approved_hash(&a, Some(&live("ravi@client.com", "Re: Else")), "Hello"), "subject");
        assert_ne!(hash, approved_hash(&a, Some(&now), "Hello there"), "body");
        assert_eq!(hash, approved_hash(&a, Some(&now), "  Hello  "), "surrounding space is not a change");
    }
}
