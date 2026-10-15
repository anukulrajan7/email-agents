use super::*;

// ---------------------------------------------------------------- mail rows

const REPLIED_SQL: &str = "(EXISTS (SELECT 1 FROM drafts d WHERE d.message_id = m.id AND d.status = 'sent')
    OR EXISTS (SELECT 1 FROM email_messages o WHERE o.thread_id = m.thread_id AND o.direction = 'outbound' AND o.suppressed = 0 AND o.received_at > m.received_at))";

fn mail_select() -> String {
    format!(
        "SELECT m.id, m.subject, m.sender_address, m.received_at, m.direction, c.category, c.priority,
                COALESCE(i.needs_reply, c.requires_reply) AS needs_reply, i.summary, {REPLIED_SQL} AS replied
         FROM email_messages m
         LEFT JOIN classifications c ON c.message_id = m.id
         LEFT JOIN message_insights i ON i.message_id = m.id"
    )
}

#[derive(sqlx::FromRow)]
pub(super) struct MailRow {
    pub(super) id: String,
    pub(super) subject: String,
    sender_address: String,
    received_at: i64,
    pub(super) direction: String,
    category: Option<String>,
    priority: Option<String>,
    needs_reply: Option<bool>,
    summary: Option<String>,
    replied: bool,
}

fn mail_json(m: &MailRow) -> Value {
    json!({
        "message_id": m.id, "subject": datamark(&m.subject, 20), "from": m.sender_address, "received_at": iso(m.received_at),
        "label": m.category, "priority": m.priority, "needs_reply": m.needs_reply, "replied": m.replied,
        "summary": m.summary.as_deref().map(|s| datamark(s, 40)),
    })
}

pub(in crate::assistant) fn text_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn limit_arg(args: &Value, default: usize, max: usize) -> usize {
    args.get("limit").and_then(Value::as_u64).map_or(default, |n| n as usize).clamp(1, max)
}

pub(super) fn like_pattern(q: &str) -> String {
    format!("%{}%", q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
}


impl Session<'_> {
    pub(super) async fn mail_by_id(&self, id: &str) -> sqlx::Result<Option<MailRow>> {
        sqlx::query_as(&format!("{} WHERE m.id = ?1 AND m.workspace_id = ?2 AND m.suppressed = 0", mail_select()))
            .bind(id)
            .bind(self.ws)
            .fetch_optional(&self.state.db)
            .await
    }


    pub(super) async fn search_mail(&mut self, args: &Value) -> ToolResult {
        let query = text_arg(args, "query").ok_or_else(|| failed("query is required"))?;
        let limit = limit_arg(args, 5, 8);
        let hits = knowledge::hybrid_search(
            &self.state.db,
            self.state.embedder.as_ref(),
            self.ws,
            &truncate_chars(query, 300),
            Scope::Documents,
            limit * 4,
        )
        .await?;
        let mut ids: Vec<String> = vec![];
        for id in hits.into_iter().filter_map(|h| h.message_id) {
            if !ids.contains(&id) {
                ids.push(id);
            }
            if ids.len() >= limit {
                break;
            }
        }
        let mut results = vec![];
        for id in ids {
            if let Some(row) = self.mail_by_id(&id).await? {
                self.see(&row.id);
                results.push(mail_json(&row));
            }
        }
        Ok(json!({ "results": results }))
    }

    pub(super) async fn list_inbox(&mut self, args: &Value) -> ToolResult {
        let filter = match text_arg(args, "filter") {
            Some("needs_reply") => format!("COALESCE(i.needs_reply, c.requires_reply) = 1 AND NOT {REPLIED_SQL}"),
            Some("important") => "c.priority = 'high'".to_string(),
            Some("unread") => format!("NOT {REPLIED_SQL} AND NOT EXISTS (SELECT 1 FROM drafts d2 WHERE d2.message_id = m.id)"),
            _ => return Err(failed("filter must be needs_reply, important or unread")),
        };
        let rows: Vec<MailRow> = sqlx::query_as(&format!(
            "{} WHERE m.workspace_id = ?1 AND m.suppressed = 0 AND m.direction = 'inbound' AND {filter} ORDER BY m.received_at DESC, m.id DESC LIMIT ?2",
            mail_select()
        ))
        .bind(self.ws)
        .bind(limit_arg(args, 5, 10) as i64)
        .fetch_all(&self.state.db)
        .await?;
        for row in &rows {
            self.see(&row.id);
        }
        Ok(json!({ "emails": rows.iter().map(mail_json).collect::<Vec<_>>() }))
    }

    pub(super) async fn summarize_email(&mut self, args: &Value) -> ToolResult {
        let id = text_arg(args, "message_id").ok_or_else(|| failed("message_id is required"))?;
        let row = self.mail_by_id(id).await?.ok_or_else(|| failed("no email with that id"))?;
        let insight = handlers::analyze_message(self.state, &json!({ "message_id": row.id })).await.map_err(|e| {
            tracing::warn!(message_id = %row.id, error = %e, "assistant summary failed");
            failed("could not read that email right now")
        })?;
        self.see(&row.id);
        let marked = |items: &[String], words: usize| items.iter().take(5).map(|s| datamark(s, words)).collect::<Vec<_>>();
        Ok(json!({
            "message_id": row.id, "subject": datamark(&row.subject, 20), "from": row.sender_address, "received_at": iso(row.received_at),
            "summary": datamark(&insight.summary, 60), "asks": marked(&insight.asks, 30),
            "deadline": insight.deadline.as_deref().map(|d| datamark(d, 10)), "needs_reply": insight.needs_reply,
            "missing_info": marked(&insight.missing_info, 30),
        }))
    }
}
