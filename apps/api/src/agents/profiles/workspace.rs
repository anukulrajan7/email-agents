use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Permissions {
    /// May the agent write reply drafts.
    pub draft: bool,
    /// Sending: `never`, `ask` (a person approves each send) or `auto` (routine replies go on their own).
    pub send: String,
}

impl Default for Permissions {
    fn default() -> Self {
        Self { draft: true, send: "ask".into() }
    }
}

impl Permissions {
    pub(super) fn parse(json: &str) -> Self {
        let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
        let send = v.get("send").and_then(Value::as_str).filter(|s| SEND_MODES.contains(s)).unwrap_or("ask");
        Self { draft: v.get("draft").and_then(Value::as_bool).unwrap_or(true), send: send.into() }
    }

    pub fn to_json(&self) -> Value {
        // Submitting candidates always needs a person; shown for clarity.
        json!({ "draft": self.draft, "send": self.send, "submit_candidate": "ask" })
    }
}

#[derive(sqlx::FromRow)]
struct AgentRow {
    workspace_id: String,
    name: String,
    avatar: String,
    color: String,
    persona: String,
    signature: Option<String>,
    tasks_json: String,
    permissions_json: String,
    quality: String,
    enabled: bool,
    updated_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceAgent {
    pub workspace_id: String,
    pub name: String,
    pub avatar: String,
    pub color: String,
    pub persona: String,
    pub signature: Option<String>,
    pub tasks: Vec<String>,
    pub permissions: Permissions,
    pub quality: String,
    pub enabled: bool,
    pub updated_at: i64,
}

impl From<AgentRow> for WorkspaceAgent {
    fn from(r: AgentRow) -> Self {
        Self {
            tasks: parse_list(&r.tasks_json, TASKS),
            permissions: Permissions::parse(&r.permissions_json),
            workspace_id: r.workspace_id,
            name: r.name,
            avatar: r.avatar,
            color: r.color,
            persona: r.persona,
            signature: r.signature.filter(|s| !s.trim().is_empty()),
            quality: r.quality,
            enabled: r.enabled,
            updated_at: r.updated_at,
        }
    }
}

impl WorkspaceAgent {
    /// True when the agent is on and does `task` (`reply` or `requirements`).
    pub fn does(&self, task: &str) -> bool {
        self.enabled && self.tasks.iter().any(|t| t == task)
    }

    /// What cases and approvals show: never the persona.
    pub fn badge(&self) -> Value {
        json!({ "name": self.name, "avatar": self.avatar, "color": self.color })
    }

    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name, "avatar": self.avatar, "color": self.color, "persona": self.persona,
            "signature": self.signature, "tasks": self.tasks, "permissions": self.permissions.to_json(),
            "quality": self.quality, "enabled": self.enabled, "updated_at": iso(self.updated_at),
        })
    }
}

/// The one rule for combining the agent's quality tier with a call's own tier: the agent's tier is
/// the floor, and a premium call can only raise it to `best`. Never lowers a tier.
pub fn effective_tier(agent_quality: &str, premium: bool) -> &'static str {
    match (agent_quality, premium) {
        ("best", _) | (_, true) => "best",
        ("fast", false) => "fast",
        _ => "balanced",
    }
}

/// The model behind a quality tier; empty when not configured (the normal models are used).
pub fn tier_model<'a>(config: &'a Config, quality: &str) -> &'a str {
    match quality {
        "fast" => &config.agent_fast_model_id,
        "best" => &config.agent_best_model_id,
        _ => &config.agent_balanced_model_id,
    }
}

/// The seeded persona: a friendly personality, the standard guidance and the workspace's own instructions.
pub fn default_persona(reply_style: &str, reply_instructions: &str) -> String {
    let base = format!("{DEFAULT_PERSONALITY}\n\n{reply_style}");
    match reply_instructions.trim() {
        "" => base,
        extra => truncate_chars(&format!("{base}\n\n{extra}"), MAX_PERSONA_CHARS),
    }
}

pub fn standard_guidance(reply_style: &str) -> String {
    default_persona(reply_style, "")
}

const AGENT_COLUMNS: &str =
    "workspace_id, name, avatar, color, persona, signature, tasks_json, permissions_json, quality, enabled, updated_at";

/// The workspace's agent, created on first use from its reply settings.
pub async fn agent(db: &SqlitePool, config: &Config, ws: &str) -> sqlx::Result<WorkspaceAgent> {
    let find = || async {
        let row: Option<AgentRow> = sqlx::query_as(&format!("SELECT {AGENT_COLUMNS} FROM workspace_agents WHERE workspace_id = ?1"))
            .bind(ws)
            .fetch_optional(db)
            .await?;
        Ok::<_, sqlx::Error>(row.map(WorkspaceAgent::from))
    };
    if let Some(a) = find().await? {
        return Ok(a);
    }
    let settings = crate::replies::reply_settings::load(db, config, ws).await?;
    let persona = default_persona(crate::platform::packs::reply_style(db, ws).await?, &settings.reply_instructions);
    let now = now_ms();
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    let created = sqlx::query(
        "INSERT OR IGNORE INTO workspace_agents (workspace_id, name, avatar, color, persona, signature, permissions_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, '{\"draft\":true,\"send\":\"auto\"}', ?7, ?7)",
    )
    .bind(ws)
    .bind(DEFAULT_NAME)
    .bind(DEFAULT_AVATAR)
    .bind(DEFAULT_COLOR)
    .bind(persona)
    .bind(settings.signature.map(|s| truncate_chars(&s, MAX_SIGNATURE_CHARS)))
    .bind(now)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if created {
        audit::record(
            &mut tx,
            Entry {
                workspace_id: ws,
                actor: Actor::System,
                action: "agent.created",
                entity_type: "workspace_agent",
                entity_id: ws,
                message_id: None,
                metadata: json!({}),
            },
        )
        .await?;
    }
    tx.commit().await?;
    find().await?.ok_or(sqlx::Error::RowNotFound)
}

async fn agent_row_send(conn: &mut sqlx::SqliteConnection, ws: &str) -> sqlx::Result<String> {
    let json: Option<String> = sqlx::query_scalar("SELECT permissions_json FROM workspace_agents WHERE workspace_id = ?1").bind(ws).fetch_optional(conn).await?;
    Ok(json
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
        .and_then(|v| v.get("send").and_then(|s| s.as_str().map(str::to_string)))
        .unwrap_or_else(|| "ask".into()))
}

pub async fn save_agent(db: &SqlitePool, ws: &str, f: &AgentFields, user_id: &str) -> sqlx::Result<()> {
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    let previous_send = agent_row_send(&mut tx, ws).await?;
    sqlx::query(
        "UPDATE workspace_agents SET name = ?1, avatar = ?2, color = ?3, persona = ?4, signature = ?5, tasks_json = ?6,
                permissions_json = ?7, quality = ?8, enabled = ?9, updated_by = ?10, updated_at = ?11
         WHERE workspace_id = ?12",
    )
    .bind(&f.name)
    .bind(&f.avatar)
    .bind(&f.color)
    .bind(&f.persona)
    .bind(&f.signature)
    .bind(json!(f.tasks).to_string())
    .bind(json!({"draft": f.permissions.draft, "send": f.permissions.send}).to_string())
    .bind(&f.quality)
    .bind(f.enabled)
    .bind(user_id)
    .bind(now_ms())
    .bind(ws)
    .execute(&mut *tx)
    .await?;
    // The registry's lead mirrors this agent (it is internal, never shown), and the workspace's send setting moves the default agents with it.
    if let Some(lead) = crate::agents::registry::store::lead(&mut *tx, ws).await? {
        crate::agents::registry::store::set_identity(&mut tx, ws, &lead.id, &f.name, &f.avatar, &f.color).await?;
    }
    crate::agents::registry::store::sync_default_agents_from_settings(&mut tx, ws, &previous_send, &f.permissions.send).await?;
    tx.commit().await
}
