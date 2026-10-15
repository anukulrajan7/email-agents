use super::*;

pub(super) struct SubmitCandidate;

impl SubmitCandidate {
    async fn build(ctx: &ToolCtx<'_>, args: &Value) -> Result<(Preview, Vec<String>), ToolError> {
        let requirement_id = str_arg(args, "requirement_id")?;
        let ids: Vec<String> = args
            .get("candidate_ids")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        let mut preview = draft_preview(ctx, str_arg(args, "draft_id")?).await?;
        let picks: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT m.candidate_id, c.name, m.score FROM matches m JOIN candidates c ON c.id = m.candidate_id
             WHERE m.requirement_id = ?1 AND m.workspace_id = ?2 AND m.status != 'dismissed'
               AND m.candidate_id IN (SELECT value FROM json_each(?3))
             ORDER BY m.score DESC",
        )
        .bind(requirement_id)
        .bind(ctx.ws())
        .bind(json!(ids).to_string())
        .fetch_all(&ctx.state.db)
        .await?;
        preview.candidates = picks.into_iter().map(|(id, name, score)| Pick { id, name, score }).collect();
        preview.unmatched_candidates = ids.iter().filter(|id| !preview.candidates.iter().any(|p| &p.id == *id)).cloned().collect();
        preview.requirement_id = Some(requirement_id.to_string());
        let n = preview.candidates.len();
        preview.summary = format!("Send submission of {n} candidate{} to {}", plural(n), preview.to.clone().unwrap_or_default());
        Ok((preview, ids))
    }
}

#[async_trait]
impl Tool for SubmitCandidate {
    fn name(&self) -> &'static str {
        "submit_candidate"
    }
    fn description(&self) -> &'static str {
        "Send the submission draft for up to 3 matched candidates to the client and mark them shortlisted. A person approves it first."
    }
    fn schema(&self) -> Value {
        object(
            json!({
                "requirement_id": {"type": "string"},
                "candidate_ids": {"type": "array", "items": {"type": "string"}},
                "draft_id": {"type": "string"}
            }),
            &["requirement_id", "candidate_ids", "draft_id"],
        )
    }
    fn risk(&self) -> Risk {
        Risk::High
    }
    async fn preview(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<Option<Preview>, ToolError> {
        Ok(Some(Self::build(ctx, args).await?.0))
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let (preview, _) = Self::build(ctx, args).await?;
        if preview.candidates.is_empty() || !preview.unmatched_candidates.is_empty() {
            return failed("Only matched candidates of this requirement can be submitted.");
        }
        let draft_id = preview.draft_id.clone().unwrap_or_default();
        let scheduled = replies::send_draft_held(ctx.state, ctx.ws(), &draft_id, ctx.actor, ctx.hold_secs).await?;
        let ids: Vec<String> = preview.candidates.iter().map(|p| p.id.clone()).collect();
        let decided_by = match ctx.actor {
            Actor::User(u) => Some(u),
            Actor::System => None,
        };
        let shortlisted = sqlx::query(
            "UPDATE matches SET status = 'shortlisted', decided_by = COALESCE(?1, decided_by), updated_at = ?2
             WHERE requirement_id = ?3 AND workspace_id = ?4 AND status != 'dismissed'
               AND candidate_id IN (SELECT value FROM json_each(?5))",
        )
        .bind(decided_by)
        .bind(now_ms())
        .bind(preview.requirement_id.as_deref())
        .bind(ctx.ws())
        .bind(json!(ids).to_string())
        .execute(&ctx.state.db)
        .await?
        .rows_affected();
        Ok(ToolOutput {
            summary: format!("Submitted {} candidate{} to {}", ids.len(), plural(ids.len()), preview.to.clone().unwrap_or_default()),
            value: json!({"scheduled": true, "draft_id": draft_id, "send_at": scheduled.send_at, "shortlisted": shortlisted}),
            state: Some(json!({"submitted_candidate_ids": ids, "last_sent_draft_id": draft_id})),
            ..Default::default()
        })
    }
}
