use super::*;

impl Session<'_> {
    pub(super) async fn list_approvals(&mut self) -> ToolResult {
        let rows = approvals::list(&self.state.db, self.ws, &self.user.id, Some("pending"), None).await?;
        let items: Vec<Value> = rows
            .iter()
            .take(10)
            .map(|a| {
                json!({
                    "approval_id": a.id, "action": a.action, "case_title": datamark(&a.case_title, 20),
                    "summary": datamark(&a.summary, 30), "to": a.to_address, "created_at": iso(a.created_at),
                })
            })
            .collect();
        Ok(json!({ "approvals": items }))
    }

    pub(super) async fn list_alerts(&mut self) -> ToolResult {
        let rows: Vec<(String, String, String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT id, kind, severity, title, status, message_id FROM alerts
             WHERE workspace_id = ?1 AND status IN ('open', 'snoozed')
             ORDER BY status = 'open' DESC, severity = 'high' DESC, created_at DESC LIMIT 10",
        )
        .bind(self.ws)
        .fetch_all(&self.state.db)
        .await?;
        let mut items = vec![];
        for (id, kind, severity, title, status, message_id) in rows {
            if let Some(m) = &message_id {
                self.see(m);
            }
            items.push(json!({
                "alert_id": id, "kind": kind, "severity": severity, "title": datamark(&title, 20), "status": status, "message_id": message_id,
            }));
        }
        Ok(json!({ "alerts": items }))
    }

    pub(super) async fn get_case(&mut self, args: &Value) -> ToolResult {
        let id = text_arg(args, "id").ok_or_else(|| failed("id is required"))?;
        let c = case::get_in(&self.state.db, self.ws, id).await?.ok_or_else(|| failed("no case with that id"))?;
        if !crate::agents::registry::store::can_see(&self.state.db, self.ws, c.agent_id.as_deref(), &self.user.id).await? {
            return Err(failed("no case with that id"));
        }
        let steps = case::steps(&self.state.db, self.ws, &c.id).await?;
        let recent: Vec<Value> = steps
            .iter()
            .skip(steps.len().saturating_sub(5))
            .map(|s| json!({"tool": s.tool, "status": s.status, "summary": datamark(&s.summary, 30)}))
            .collect();
        let pending: Vec<Value> = approvals::list(&self.state.db, self.ws, &self.user.id, Some("pending"), Some(&c.id))
            .await?
            .iter()
            .map(|a| json!({"approval_id": a.id, "action": a.action}))
            .collect();
        if let Some(m) = &c.message_id {
            self.see(m);
        }
        Ok(json!({
            "case_id": c.id, "kind": c.kind, "title": datamark(&c.title, 20), "goal": datamark(&c.goal, 30), "status": c.status,
            "outcome": c.outcome, "message_id": c.message_id, "recent_steps": recent, "pending_approvals": pending,
        }))
    }

    pub(super) async fn list_matches(&mut self, args: &Value) -> ToolResult {
        if !self.features.matching {
            return Err(failed("unknown tool"));
        }
        let key = text_arg(args, "requirement").ok_or_else(|| failed("requirement is required"))?;
        let found: Option<(String, Option<String>, String, String)> = sqlx::query_as(
            "SELECT id, client, role_title, message_id FROM requirements
             WHERE workspace_id = ?1 AND (id = ?2 OR role_title LIKE ?3 ESCAPE '\\' OR client LIKE ?3 ESCAPE '\\')
             ORDER BY id = ?2 DESC, status = 'open' DESC, created_at DESC LIMIT 1",
        )
        .bind(self.ws)
        .bind(key)
        .bind(like_pattern(key))
        .fetch_optional(&self.state.db)
        .await?;
        let (id, client, role_title, message_id) = found.ok_or_else(|| failed("no requirement matches that"))?;
        let rows: Vec<(String, String, i64, String)> = sqlx::query_as(
            "SELECT x.id, c.name, x.score, x.status FROM matches x JOIN candidates c ON c.id = x.candidate_id
             WHERE x.requirement_id = ?1 AND x.workspace_id = ?2 AND x.status != 'dismissed' ORDER BY x.score DESC LIMIT 8",
        )
        .bind(&id)
        .bind(self.ws)
        .fetch_all(&self.state.db)
        .await?;
        self.see(&message_id);
        Ok(json!({
            "requirement": {
                "requirement_id": id, "role": datamark(&role_title, 15), "client": client.as_deref().map(|c| datamark(c, 10)),
                "message_id": message_id,
            },
            "matches": rows.iter().map(|(mid, name, score, status)| json!({
                "match_id": mid, "candidate": datamark(name, 6), "score": score, "status": status,
            })).collect::<Vec<_>>(),
        }))
    }


    pub(super) async fn list_conversations(&mut self) -> ToolResult {
        let items = messaging::watch::list(self.state, self.ws, &self.user.id).await?;
        let rows: Vec<Value> = items
            .iter()
            .map(|i| json!({
                "id": i["id"], "kind": i["kind"], "title": datamark(i["title"].as_str().unwrap_or_default(), 20), "with": i["with"],
                "status": i["status_label"], "answer": i["answer"].as_str().map(|a| datamark(a, 40)),
            }))
            .collect();
        Ok(json!({ "conversations": rows }))
    }
}
