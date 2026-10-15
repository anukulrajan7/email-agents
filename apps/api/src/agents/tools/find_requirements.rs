use super::*;

pub(super) struct FindRequirements;

#[derive(sqlx::FromRow)]
struct OpenRequirementRow {
    id: String,
    role_title: String,
    location: Option<String>,
    must_have_skills_json: String,
    positions: Option<i64>,
}

#[async_trait]
impl Tool for FindRequirements {
    fn name(&self) -> &'static str {
        "find_requirements"
    }
    fn description(&self) -> &'static str {
        "Open job requirements (current openings), optionally filtered by skill, role or location. Use it to answer a job seeker who asks what is open instead of asking a person; an empty result means there is no such opening."
    }
    fn schema(&self) -> Value {
        object(
            json!({
                "skill": {"type": "string", "description": "A skill, e.g. React Native"},
                "role": {"type": "string", "description": "Part of the role title"},
                "location": {"type": "string"},
                "limit": {"type": "integer", "description": "1 to 10, default 5"}
            }),
            &[],
        )
    }
    fn risk(&self) -> Risk {
        Risk::Read
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let rows: Vec<OpenRequirementRow> = sqlx::query_as(
            "SELECT id, role_title, location, must_have_skills_json, positions FROM requirements
             WHERE workspace_id = ?1 AND status = 'open'
               AND (?2 IS NULL OR LOWER(must_have_skills_json) LIKE '%' || LOWER(?2) || '%' OR LOWER(nice_to_have_skills_json) LIKE '%' || LOWER(?2) || '%')
               AND (?3 IS NULL OR LOWER(role_title) LIKE '%' || LOWER(?3) || '%')
               AND (?4 IS NULL OR LOWER(COALESCE(location, '')) LIKE '%' || LOWER(?4) || '%')
             ORDER BY created_at DESC LIMIT ?5",
        )
        .bind(ctx.ws())
        .bind(opt_str(args, "skill").map(str::trim).filter(|s| !s.is_empty()))
        .bind(opt_str(args, "role").map(str::trim).filter(|s| !s.is_empty()))
        .bind(opt_str(args, "location").map(str::trim).filter(|s| !s.is_empty()))
        .bind(limit_arg(args, 5, 10))
        .fetch_all(&ctx.state.db)
        .await?;
        let items: Vec<Value> = rows
            .iter()
            .map(|r| {
                let skills: Vec<String> = serde_json::from_str(&r.must_have_skills_json).unwrap_or_default();
                json!({"requirement_id": r.id, "role": r.role_title, "location": r.location, "skills": skills, "positions": r.positions})
            })
            .collect();
        let summary = match rows.len() {
            0 => "No open requirement matches".to_string(),
            n => format!("Found {n} open requirement{}", plural(n)),
        };
        Ok(ToolOutput { summary, value: json!({"items": items}), ..Default::default() })
    }
}
