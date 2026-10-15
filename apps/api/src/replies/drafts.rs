use super::*;

/// Saves (or updates) the reply in the mailbox's Drafts folder. Returns the provider draft id.
pub async fn save_draft(state: &AppState, ws: &str, draft_id: &str, actor: Actor<'_>) -> AppResult<String> {
    let ctx = load(state, ws, draft_id).await?;
    if !policy::can_create_provider_draft(&capabilities(&ctx)) {
        return Err(AppError::Conflict("this mailbox can't hold drafts; copy the text instead".into()));
    }
    if ctx.status == "sent" {
        return Err(AppError::Conflict("this reply was already sent".into()));
    }
    if ctx.send_at.is_some() || ctx.send_attempt_id.is_some() {
        return Err(AppError::Conflict("this reply is being sent; undo the send first".into()));
    }
    let (conn, pck) = connection(&ctx)?;
    let message = raw_for(state, &ctx, conn, pck, None).await?;
    let result = match &ctx.provider_draft_id {
        Some(existing) => {
            state.nango.gmail_call(Method::PUT, conn, pck, &format!("drafts/{existing}"), Some(&json!({ "message": message }))).await
        }
        None => state.nango.gmail_call(Method::POST, conn, pck, "drafts", Some(&json!({ "message": message }))).await,
    }?;
    let provider_draft_id = result.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("UPDATE drafts SET provider_draft_id = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(&provider_draft_id)
        .bind(now_ms())
        .bind(&ctx.draft_id)
        .execute(&mut *tx)
        .await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor,
            action: "draft.saved_to_mailbox",
            entity_type: "draft",
            entity_id: &ctx.draft_id,
            message_id: Some(&ctx.message_id),
            metadata: json!({ "provider_draft_id": provider_draft_id }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(provider_draft_id)
}

pub async fn save_to_provider(
    State(state): State<AppState>,
    user: AuthUser,
    Path((ws, id)): Path<(String, String)>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    crate::hr::team::scope::require_draft(&state.db, &ws, &user, &id).await?;
    let provider_draft_id = save_draft(&state, &ws, &id, Actor::User(&user.id)).await?;
    Ok(Json(json!({ "status": "saved", "provider_draft_id": provider_draft_id })))
}
