use super::*;

type TicketBrief = (String, String, String, String, String);

pub async fn list(state: &AppState, ws: &str, user_id: &str) -> sqlx::Result<Vec<Value>> {
    let watches: Vec<Watch> = sqlx::query_as(&format!(
        "SELECT {WATCH_COLUMNS} FROM watches WHERE workspace_id = ?1 AND user_id = ?2 AND status != 'closed' ORDER BY last_update_at DESC, id DESC LIMIT 50"
    ))
    .bind(ws)
    .bind(user_id)
    .fetch_all(&state.db)
    .await?;
    let mut out = vec![];
    for w in watches {
        let ticket: Option<TicketBrief> = sqlx::query_as(
            "SELECT id, company, support_address, status, last_summary FROM support_tickets WHERE workspace_id = ?1 AND watch_id = ?2",
        )
        .bind(ws)
        .bind(&w.id)
        .fetch_optional(&state.db)
        .await?;
        let (label, key) = match (w.status.as_str(), ticket.as_ref().map(|t| t.3.as_str())) {
            (_, Some("resolved")) => ("Resolved", "resolved"),
            ("replied", Some("open")) => ("They replied: needs a look", "open"),
            (_, Some("open")) => ("About to write to them", "starting"),
            ("replied", _) => ("Answered", "answered"),
            ("nudged", _) => ("No answer yet: I nudged once", "nudged"),
            _ => ("Waiting for a reply", "waiting"),
        };
        out.push(json!({
            "id": w.id, "kind": w.kind, "title": w.title, "with": w.counterpart, "channel": w.channel,
            "status": key, "status_label": label, "answer": w.answer_text, "updated_at": iso(w.last_update_at),
            "ticket": ticket.map(|t| json!({ "id": t.0, "company": t.1, "address": t.2, "status": t.3, "last_summary": t.4 })),
        }));
    }
    Ok(out)
}

/// The person stops watching (and, for a ticket, calls it resolved).
pub async fn close(state: &AppState, ws: &str, user_id: &str, id: &str) -> AppResult<()> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let closed = sqlx::query("UPDATE watches SET status = 'closed', last_update_at = ?1 WHERE id = ?2 AND workspace_id = ?3 AND user_id = ?4 AND status != 'closed'")
        .bind(now_ms())
        .bind(id)
        .bind(ws)
        .bind(user_id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        == 1;
    if !closed {
        return Err(AppError::NotFound);
    }
    sqlx::query("UPDATE support_tickets SET status = 'resolved', last_update_at = ?1 WHERE workspace_id = ?2 AND watch_id = ?3")
        .bind(now_ms())
        .bind(ws)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    audit::record(
        &mut tx,
        Entry { workspace_id: ws, actor: Actor::User(user_id), action: "message.watch_closed", entity_type: "watch", entity_id: id, message_id: None, metadata: json!({}) },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
