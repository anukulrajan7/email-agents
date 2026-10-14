//! `POST /assistant/confirm`: runs one plan card a person confirmed, through the same code
//! paths as the matching screens. A card is user-bound, expires, and works once.

use serde_json::{Value, json};

use crate::{
    agents::{approvals, triggers},
    audit::{self, Actor, Entry},
    auth::{AuthUser, require_role},
    automations::{self, AutomationInput},
    db::{new_id, now_ms},
    domain::Role,
    error::{AppError, AppResult},
    http::AppState,
    jobs::{self, kind},
};

#[derive(sqlx::FromRow)]
struct PlanRow {
    user_id: String,
    action: String,
    params_json: String,
    expires_at: i64,
    used_at: Option<i64>,
}

struct Done {
    message: String,
    link: Option<Value>,
}

fn done(message: &str, link: Option<Value>) -> Done {
    Done { message: message.into(), link }
}

fn str_param<'a>(params: &'a Value, key: &str) -> AppResult<&'a str> {
    params.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| AppError::BadRequest(format!("the plan is missing {key}")))
}

pub async fn confirm(state: &AppState, ws: &str, user: &AuthUser, plan_item_id: &str) -> AppResult<Value> {
    let row = sqlx::query_as::<_, PlanRow>(
        "SELECT user_id, action, params_json, expires_at, used_at FROM assistant_plans WHERE id = ?1 AND workspace_id = ?2",
    )
    .bind(plan_item_id)
    .bind(ws)
    .fetch_optional(&state.db)
    .await?
    .filter(|r| r.user_id == user.id)
    .ok_or(AppError::NotFound)?;
    if row.used_at.is_some() {
        return Err(AppError::Conflict("this action was already done".into()));
    }
    let now = now_ms();
    if row.expires_at <= now {
        return Err(AppError::Conflict("this action expired; ask again".into()));
    }
    require_role(&state.db, ws, user, if row.action == "create_automation" { Role::Admin } else { Role::Operator }).await?;

    let claimed = sqlx::query("UPDATE assistant_plans SET used_at = ?1 WHERE id = ?2 AND used_at IS NULL AND expires_at > ?1")
        .bind(now)
        .bind(plan_item_id)
        .execute(&state.db)
        .await?
        .rows_affected()
        == 1;
    if !claimed {
        return Err(AppError::Conflict("this action was already done".into()));
    }

    let params: Value = serde_json::from_str(&row.params_json).unwrap_or_else(|_| json!({}));
    let outcome = match row.action.as_str() {
        "draft_reply" => draft_reply(state, ws, user, &params).await,
        "start_agent" => start_agent(state, ws, user, &params).await,
        "approve" => decide(state, ws, user, &params, true).await,
        "decline" => decide(state, ws, user, &params, false).await,
        "create_automation" => create_automation(state, ws, user, &params).await,
        "snooze_alert" => snooze_alert(state, ws, user, &params).await,
        _ => Err(AppError::BadRequest("unknown action".into())),
    };
    let Done { message, link } = match outcome {
        Ok(done) => done,
        Err(e) => {
            sqlx::query("UPDATE assistant_plans SET used_at = NULL WHERE id = ?1").bind(plan_item_id).execute(&state.db).await?;
            return Err(e);
        }
    };

    let mut conn = state.db.acquire().await?;
    audit::record(
        &mut conn,
        Entry {
            workspace_id: ws,
            actor: Actor::User(&user.id),
            action: "assistant.confirmed",
            entity_type: "assistant_plan",
            entity_id: plan_item_id,
            message_id: None,
            metadata: json!({ "action": row.action }),
        },
    )
    .await?;
    Ok(json!({ "ok": true, "action": row.action, "message": message, "link": link }))
}

async fn draft_reply(state: &AppState, ws: &str, user: &AuthUser, params: &Value) -> AppResult<Done> {
    let message_id = str_param(params, "message_id")?;
    let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM email_messages WHERE id = ?1 AND workspace_id = ?2")
        .bind(message_id)
        .bind(ws)
        .fetch_optional(&state.db)
        .await?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }
    let instructions = params.get("instructions").and_then(Value::as_str).filter(|i| !i.trim().is_empty());
    let mut payload = json!({ "message_id": message_id });
    if let Some(i) = instructions {
        payload["instructions"] = json!(i);
    }
    let key = format!("draft:{message_id}:manual:{}", new_id("req"));
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    jobs::enqueue(&mut tx, kind::GENERATE_DRAFT, &key, Some(ws), &payload).await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor: Actor::User(&user.id),
            action: "draft.requested",
            entity_type: "message",
            entity_id: message_id,
            message_id: Some(message_id),
            metadata: json!({ "premium": false, "via": "assistant" }),
        },
    )
    .await?;
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(done("Drafting the reply. It will appear on the email shortly.", Some(json!({ "kind": "message", "id": message_id }))))
}

async fn start_agent(state: &AppState, ws: &str, user: &AuthUser, params: &Value) -> AppResult<Done> {
    if !state.planner.enabled() {
        return Err(AppError::Conflict("agents are not set up yet; ask an admin to configure the AI service".into()));
    }
    let started = match str_param(params, "kind")? {
        "requirement" => triggers::start_requirement_case(state, ws, str_param(params, "requirement_id")?, Some(&user.id)).await?,
        "inbox" => triggers::start_inbox_case(state, ws, str_param(params, "message_id")?, Some(&user.id)).await?,
        _ => return Err(AppError::BadRequest("kind must be requirement or inbox".into())),
    };
    let case_id = match started {
        triggers::Started::Case(id, _) => id,
        triggers::Started::NotFound => return Err(AppError::NotFound),
        triggers::Started::NotAllowed(why) => return Err(AppError::Conflict(why)),
    };
    Ok(done("Agent started. Follow it on the Agents page.", Some(json!({ "kind": "case", "id": case_id }))))
}

async fn decide(state: &AppState, ws: &str, user: &AuthUser, params: &Value, approve: bool) -> AppResult<Done> {
    let id = str_param(params, "approval_id")?;
    let note = params.get("note").and_then(Value::as_str).filter(|n| !n.trim().is_empty());
    approvals::decide(state, ws, id, &user.id, approvals::Decision { approve, edited_body: None, note }).await?;
    Ok(done(if approve { "Approved." } else { "Declined." }, Some(json!({ "kind": "approvals" }))))
}

async fn create_automation(state: &AppState, ws: &str, user: &AuthUser, params: &Value) -> AppResult<Done> {
    let input: AutomationInput =
        serde_json::from_value(params.clone()).map_err(|e| AppError::BadRequest(format!("invalid automation: {e}")))?;
    automations::validate(&input.name, &input.conditions, &input.actions).map_err(AppError::BadRequest)?;
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let id = automations::insert(&mut tx, ws, &input, None).await?;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM automations WHERE workspace_id = ?1").bind(ws).fetch_one(&mut *tx).await?;
    if total > automations::MAX_AUTOMATIONS {
        return Err(AppError::BadRequest(format!("at most {} automations per workspace", automations::MAX_AUTOMATIONS)));
    }
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor: Actor::User(&user.id),
            action: "automation.created",
            entity_type: "automation",
            entity_id: &id,
            message_id: None,
            metadata: json!({ "preset_key": null, "enabled": input.enabled, "via": "assistant" }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(done("Automation created.", Some(json!({ "kind": "automations" }))))
}

async fn snooze_alert(state: &AppState, ws: &str, user: &AuthUser, params: &Value) -> AppResult<Done> {
    let id = str_param(params, "alert_id")?;
    let hours = params.get("hours").and_then(Value::as_i64).filter(|h| (1..=168).contains(h)).ok_or_else(|| AppError::BadRequest("hours must be 1 to 168".into()))?;
    let now = now_ms();
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let updated = sqlx::query(
        "UPDATE alerts SET status = 'snoozed', snoozed_until = ?1 + ?2 * 3600000, resolved_at = NULL, resolved_by = NULL, updated_at = ?1
         WHERE id = ?3 AND workspace_id = ?4 AND status != 'resolved'",
    )
    .bind(now)
    .bind(hours)
    .bind(id)
    .bind(ws)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if updated == 0 {
        return Err(AppError::NotFound);
    }
    let message_id: Option<String> = sqlx::query_scalar("SELECT message_id FROM alerts WHERE id = ?1").bind(id).fetch_one(&mut *tx).await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor: Actor::User(&user.id),
            action: "alert.snoozed",
            entity_type: "alert",
            entity_id: id,
            message_id: message_id.as_deref(),
            metadata: json!({ "snooze_hours": hours, "via": "assistant" }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(done(&format!("Alert snoozed for {hours} hours."), Some(json!({ "kind": "alerts" }))))
}
