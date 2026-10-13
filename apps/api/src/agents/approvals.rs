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
}

const APPROVAL_SELECT: &str = "SELECT a.id, a.case_id, c.title AS case_title, a.step_seq, a.action, a.summary, a.to_address, a.subject,
        a.body, a.draft_id, a.candidates_json, a.input_json, a.status, u.email AS decided_by_email, a.decided_at, a.created_at,
        c.agent_user_id
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

pub async fn list(db: &SqlitePool, ws: &str, status: Option<&str>, case_id: Option<&str>) -> sqlx::Result<Vec<ApprovalRow>> {
    sqlx::query_as(&format!(
        "{APPROVAL_SELECT} WHERE a.workspace_id = ?1 AND (?2 IS NULL OR a.status = ?2) AND (?3 IS NULL OR a.case_id = ?3)
         ORDER BY a.created_at DESC LIMIT 200"
    ))
    .bind(ws)
    .bind(status)
    .bind(case_id)
    .fetch_all(db)
    .await
}

pub async fn get(db: &SqlitePool, ws: &str, id: &str) -> sqlx::Result<Option<ApprovalRow>> {
    sqlx::query_as(&format!("{APPROVAL_SELECT} WHERE a.id = ?1 AND a.workspace_id = ?2")).bind(id).bind(ws).fetch_optional(db).await
}

pub async fn pending_count(db: &SqlitePool, ws: &str) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT COUNT(*) FROM agent_approvals WHERE workspace_id = ?1 AND status = 'pending'").bind(ws).fetch_one(db).await
}

pub struct Decision<'a> {
    pub approve: bool,
    pub edited_body: Option<&'a str>,
    pub note: Option<&'a str>,
}

/// Records a person's decision, applies an approval, and lets the case continue. Audited.
pub async fn decide(state: &AppState, ws: &str, approval_id: &str, user_id: &str, d: Decision<'_>) -> AppResult<ApprovalRow> {
    let approval = get(&state.db, ws, approval_id).await?.ok_or(AppError::NotFound)?;
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

    let mut tx = state.db.begin().await?;
    let decided = sqlx::query(
        "UPDATE agent_approvals SET status = ?1, decided_by = ?2, decided_at = ?3, note = ?4, body = COALESCE(?5, body)
         WHERE id = ?6 AND status = 'pending'",
    )
    .bind(if d.approve { "approved" } else { "declined" })
    .bind(user_id)
    .bind(now)
    .bind(&note)
    .bind(&edited)
    .bind(approval_id)
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
    tx.commit().await?;

    // Apply outside any transaction (the send is a network call); its own guards still hold.
    let step = if d.approve {
        let tool = tools::registry().get(tools::tool_for_approval(&approval.action)).ok_or_else(|| AppError::Internal(anyhow::anyhow!("unknown approval action")))?;
        let args: Value = serde_json::from_str(&approval.input_json).unwrap_or_else(|_| json!({}));
        let ctx = ToolCtx { state, case: &case, turn: case.step_count, actor: Actor::User(user_id) };
        match tool.run(&ctx, &args).await {
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
        case::merge_state(&mut tx, &case.id, patch).await?;
    }
    if !d.approve && let Some(draft_id) = &approval.draft_id {
        case::merge_state(&mut tx, &case.id, &json!({"declined_draft_id": draft_id})).await?;
    }
    if case::set_status(&mut tx, &case.id, "running").await? {
        case::enqueue_step(&mut tx, ws, &case.id, case.step_count + 1, now_ms()).await?;
    }
    tx.commit().await?;
    state.jobs_notify.notify_one();
    get(&state.db, ws, approval_id).await?.ok_or(AppError::NotFound)
}
