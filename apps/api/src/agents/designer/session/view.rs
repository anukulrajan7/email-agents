use super::*;

/// The session as the web app reads it.
pub async fn view(state: &AppState, ws: &str, s: &Session) -> AppResult<Value> {
    let diff = match (&s.agent_id, &s.spec) {
        (Some(agent_id), Some(spec)) => match store::get_in(&state.db, ws, agent_id).await? {
            Some(agent) => {
                let (current, _) = versions::active_spec_in(&state.db, &agent).await?;
                Some(versions::diff(&current, spec))
            }
            None => None,
        },
        _ => None,
    };
    let dry_run = match s.dry_run_status.as_str() {
        "idle" => Value::Null,
        "running" => json!({"status": "running"}),
        status => {
            let mut v = s.dry_run.clone().unwrap_or_else(|| json!({}));
            v["status"] = json!(status);
            v
        }
    };
    let routing = s.routing_preview.clone().or_else(|| s.spec.as_ref().map(validate::routing_preview));
    Ok(json!({
        "warnings": s.warnings, "routing_preview": routing,
        "id": s.id, "goal": s.goal, "agent_id": s.agent_id, "base_version": s.base_version, "status": s.status,
        "turns": s.turns, "spec": s.spec, "issues": s.issues,
        "questions": s.spec.as_ref().map(|x| x.questions_for_user.clone()).unwrap_or_default(),
        "assumptions": s.spec.as_ref().map(|x| x.assumptions.clone()).unwrap_or_default(),
        "diff": diff, "dry_run": dry_run, "cost_usd": (s.cost_usd * 1e6).round() / 1e6,
        "limits": {"calls_today": calls_today(&state.db, ws).await?, "daily_limit": DAILY_LIMIT, "session_budget_usd": SESSION_BUDGET_USD},
        "created_at": iso(s.created_at), "updated_at": iso(s.updated_at),
    }))
}

/// Opens a designer session from an agent's suggested automation, so an admin can review it,
/// dry-run it and save it as a real agent. Nothing runs until they save. If the designer cannot
/// answer right now (limit, no model) the session still opens with the proposal as its description.
pub async fn open_from_proposal(state: &AppState, ws: &str, user_id: &str, name: &str, description: &str) -> AppResult<Session> {
    let role: Option<String> = sqlx::query_scalar("SELECT role FROM memberships WHERE workspace_id = ?1 AND user_id = ?2").bind(ws).bind(user_id).fetch_optional(&state.db).await?;
    if role.as_deref() != Some("admin") {
        return Err(AppError::Forbidden);
    }
    let session = create(state, ws, user_id, description, None).await?;
    match design_turn(state, ws, &session.id, description).await {
        Ok(s) => Ok(s),
        Err(AppError::LimitReached(_) | AppError::AiUnavailable) => {
            tracing::info!(session_id = %session.id, %name, "designer session opened without a first draft");
            Ok(session)
        }
        Err(e) => Err(e),
    }
}
