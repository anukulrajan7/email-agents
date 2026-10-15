use super::*;

pub(super) struct FindCandidates;

#[derive(sqlx::FromRow)]
struct MatchRow {
    candidate_id: String,
    name: String,
    score: i64,
    status: String,
    reasons_json: String,
}

#[async_trait]
impl Tool for FindCandidates {
    fn name(&self) -> &'static str {
        "find_candidates"
    }
    fn description(&self) -> &'static str {
        "Best matching candidates for a requirement, with score (0-100) and reasons."
    }
    fn schema(&self) -> Value {
        object(json!({"requirement_id": {"type": "string"}, "limit": {"type": "integer", "description": "1 to 10, default 3"}}), &["requirement_id"])
    }
    fn risk(&self) -> Risk {
        Risk::Read
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let requirement_id = str_arg(args, "requirement_id")?;
        let exists: Option<String> = sqlx::query_scalar("SELECT id FROM requirements WHERE id = ?1 AND workspace_id = ?2")
            .bind(requirement_id)
            .bind(ctx.ws())
            .fetch_optional(&ctx.state.db)
            .await?;
        if exists.is_none() {
            return failed("That requirement was not found in this workspace.");
        }
        let rows: Vec<MatchRow> = sqlx::query_as(
            "SELECT m.candidate_id, c.name, m.score, m.status, m.reasons_json
             FROM matches m JOIN candidates c ON c.id = m.candidate_id
             WHERE m.requirement_id = ?1 AND m.workspace_id = ?2 AND m.status != 'dismissed'
             ORDER BY m.score DESC, m.created_at LIMIT ?3",
        )
        .bind(requirement_id)
        .bind(ctx.ws())
        .bind(limit_arg(args, 3, 10))
        .fetch_all(&ctx.state.db)
        .await?;
        let items: Vec<Value> = rows
            .iter()
            .map(|r| {
                let reasons: Vec<String> = serde_json::from_str::<Vec<Value>>(&r.reasons_json)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|x| x.get("text").and_then(Value::as_str).map(str::to_string))
                    .collect();
                json!({"candidate_id": r.candidate_id, "name": r.name, "score": r.score, "status": r.status, "reasons": reasons})
            })
            .collect();
        let summary = match rows.first() {
            None => "No matching candidates yet".to_string(),
            Some(best) => format!("Found {} candidate{} (best {})", rows.len(), plural(rows.len()), best.score),
        };
        let picks: Vec<Pick> = rows.iter().map(|r| Pick { id: r.candidate_id.clone(), name: r.name.clone(), score: r.score }).collect();
        Ok(ToolOutput {
            summary,
            state: Some(json!({"candidates": picks, "candidate_ids": picks.iter().map(|p| p.id.clone()).collect::<Vec<_>>()})),
            value: json!({"items": items}),
            ..Default::default()
        })
    }
}
