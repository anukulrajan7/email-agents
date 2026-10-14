//! The workspace's agent and each person's profile. There is one agent per workspace (name,
//! persona, jobs, permissions, quality tier); it adapts to whoever owns the mailbox an email
//! arrived in by reading that person's profile. Both are team-written, so they are trusted in
//! prompts, but they always come after the fixed safety rules, which win.

use std::future::Future;

use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::{
    ai::routing::DEFAULT_REPLY_STYLE,
    audit::{self, Actor, Entry},
    config::Config,
    db::{iso, now_ms},
    sources::normalize::truncate_chars,
};

pub const MAX_NAME_CHARS: usize = 60;
pub const MAX_PERSONA_CHARS: usize = 3_000;
pub const MAX_SIGNATURE_CHARS: usize = 200;
pub const TASK_REPLY: &str = "reply";
pub const TASK_REQUIREMENTS: &str = "requirements";
pub const TASKS: &[&str] = &[TASK_REPLY, TASK_REQUIREMENTS];
pub const COLORS: &[&str] = &["slate", "sage", "plum", "ochre", "teal", "postmark", "ink"];
pub const QUALITIES: &[&str] = &["fast", "balanced", "best"];
pub const SEND_MODES: &[&str] = &["never", "ask", "auto"];

pub const DEFAULT_NAME: &str = "Meera";
pub const DEFAULT_AVATAR: &str = "🌼";
pub const DEFAULT_COLOR: &str = "ochre";
pub(crate) const DEFAULT_PERSONALITY: &str = "I'm Meera, the team's email assistant. I'm calm and organised, \
and always professional: friendly greetings, clear next steps, no jargon, and never more words than needed.";

// ---------------------------------------------------------------- the workspace agent

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
    fn parse(json: &str) -> Self {
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
    let settings = crate::reply_settings::load(db, config, ws).await?;
    let persona = default_persona(crate::packs::reply_style(db, ws).await?, &settings.reply_instructions);
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
    if let Some(lead) = crate::agent_registry::store::lead(&mut *tx, ws).await? {
        crate::agent_registry::store::set_identity(&mut tx, ws, &lead.id, &f.name, &f.avatar, &f.color).await?;
    }
    crate::agent_registry::store::sync_default_agents_from_settings(&mut tx, ws, &previous_send, &f.permissions.send).await?;
    tx.commit().await
}

// ---------------------------------------------------------------- people's profiles

pub const HANDLES: &[&str] = &["client_requirements", "candidates", "timesheets", "leave", "payroll", "onboarding", "general"];
pub const AUDIENCES: &[&str] = &["clients", "candidates", "colleagues", "vendors"];
pub const FORMALITIES: &[&str] = &["casual", "friendly", "formal"];
pub const LENGTHS: &[&str] = &["short", "medium", "detailed"];
pub const SEND_APPROVALS: &[&str] = &["always_ask", "routine_ok"];
pub const MAX_VIPS: usize = 20;

/// How the person wants replies to sound. `sign_off` is their signature.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tone {
    pub formality: String,
    pub length: String,
    pub sign_off: Option<String>,
    pub language: String,
}

/// Approval preference per action type. Only sending can be relaxed; submissions always ask.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileApprovals {
    pub send_email: String,
}

impl Default for ProfileApprovals {
    fn default() -> Self {
        Self { send_email: "routine_ok".into() }
    }
}

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
struct UserRow {
    display_name: String,
    job_title: String,
    handles_json: String,
    handles_note: String,
    ignore_note: String,
    audiences_json: String,
    vips_json: String,
    formality: String,
    length: String,
    language: String,
    signature: Option<String>,
    working_hours: String,
    timezone: String,
    never_do: String,
    approvals_json: String,
    saved_at: Option<i64>,
    skipped_at: Option<i64>,
    updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UserProfile {
    pub display_name: String,
    pub job_title: String,
    /// What they handle (checkboxes) and in their own words.
    pub handles: Vec<String>,
    pub handles_note: String,
    /// What the agent should leave alone for them.
    pub ignore_note: String,
    /// Who they usually email.
    pub audiences: Vec<String>,
    /// Important people and domains (addresses or domains, lower-case).
    pub vips: Vec<String>,
    pub tone: Tone,
    pub working_hours: String,
    pub timezone: String,
    pub never_do: String,
    pub approvals: ProfileApprovals,
    pub saved: bool,
    pub skipped: bool,
    pub updated_at: Option<i64>,
}

impl From<UserRow> for UserProfile {
    fn from(r: UserRow) -> Self {
        let approvals: Value = serde_json::from_str(&r.approvals_json).unwrap_or(Value::Null);
        let send_email = approvals.get("send_email").and_then(Value::as_str).filter(|s| SEND_APPROVALS.contains(s)).unwrap_or("always_ask");
        Self {
            handles: parse_list(&r.handles_json, HANDLES),
            audiences: parse_list(&r.audiences_json, AUDIENCES),
            vips: serde_json::from_str(&r.vips_json).unwrap_or_default(),
            display_name: r.display_name,
            job_title: r.job_title,
            handles_note: r.handles_note,
            ignore_note: r.ignore_note,
            tone: Tone {
                formality: r.formality,
                length: r.length,
                sign_off: r.signature.filter(|s| !s.trim().is_empty()),
                language: r.language,
            },
            working_hours: r.working_hours,
            timezone: r.timezone,
            never_do: r.never_do,
            approvals: ProfileApprovals { send_email: send_email.into() },
            saved: r.saved_at.is_some(),
            skipped: r.skipped_at.is_some(),
            updated_at: Some(r.updated_at),
        }
    }
}

impl UserProfile {
    pub fn blank() -> Self {
        Self::default()
    }

    /// The person lets routine replies go without asking.
    pub fn routine_ok(&self) -> bool {
        !self.saved || self.approvals.send_email == "routine_ok"
    }

    pub fn to_json(&self) -> Value {
        json!({
            "display_name": self.display_name, "job_title": self.job_title, "handles": self.handles, "handles_note": self.handles_note,
            "ignore_note": self.ignore_note, "audiences": self.audiences, "vips": self.vips,
            "tone": { "formality": self.tone.formality, "length": self.tone.length, "sign_off": self.tone.sign_off, "language": self.tone.language },
            "working_hours": self.working_hours, "timezone": self.timezone, "never_do": self.never_do,
            "approvals": { "send_email": self.approvals.send_email, "submit_candidate": "always_ask" },
            "saved": self.saved, "updated_at": self.updated_at.map(iso),
        })
    }
}

const USER_COLUMNS: &str = "display_name, job_title, handles_json, handles_note, ignore_note, audiences_json, vips_json, formality, length,
    language, signature, working_hours, timezone, never_do, approvals_json, saved_at, skipped_at, updated_at";

pub async fn user_profile(db: &SqlitePool, ws: &str, user_id: &str) -> sqlx::Result<Option<UserProfile>> {
    let row: Option<UserRow> =
        sqlx::query_as(&format!("SELECT {USER_COLUMNS} FROM user_agent_profiles WHERE workspace_id = ?1 AND user_id = ?2"))
            .bind(ws)
            .bind(user_id)
            .fetch_optional(db)
            .await?;
    Ok(row.map(Into::into))
}

pub async fn save_user_profile(db: &SqlitePool, ws: &str, user_id: &str, p: &UserProfile) -> sqlx::Result<()> {
    let now = now_ms();
    sqlx::query(
        "INSERT INTO user_agent_profiles (workspace_id, user_id, display_name, job_title, handles_json, handles_note, ignore_note,
                                          audiences_json, vips_json, formality, length, language, signature, working_hours, timezone,
                                          never_do, approvals_json, saved_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?18, ?18)
         ON CONFLICT(workspace_id, user_id) DO UPDATE SET
           display_name = excluded.display_name, job_title = excluded.job_title, handles_json = excluded.handles_json,
           handles_note = excluded.handles_note, ignore_note = excluded.ignore_note, audiences_json = excluded.audiences_json,
           vips_json = excluded.vips_json, formality = excluded.formality, length = excluded.length, language = excluded.language,
           signature = excluded.signature, working_hours = excluded.working_hours, timezone = excluded.timezone,
           never_do = excluded.never_do, approvals_json = excluded.approvals_json,
           saved_at = excluded.saved_at, updated_at = excluded.updated_at",
    )
    .bind(ws)
    .bind(user_id)
    .bind(&p.display_name)
    .bind(&p.job_title)
    .bind(json!(p.handles).to_string())
    .bind(&p.handles_note)
    .bind(&p.ignore_note)
    .bind(json!(p.audiences).to_string())
    .bind(json!(p.vips).to_string())
    .bind(&p.tone.formality)
    .bind(&p.tone.length)
    .bind(&p.tone.language)
    .bind(&p.tone.sign_off)
    .bind(&p.working_hours)
    .bind(&p.timezone)
    .bind(&p.never_do)
    .bind(json!({ "send_email": p.approvals.send_email }).to_string())
    .bind(now)
    .execute(db)
    .await?;
    Ok(())
}

/// "Skip for now" in onboarding: remembered so the prompt doesn't come back.
pub async fn skip_user_profile(db: &SqlitePool, ws: &str, user_id: &str) -> sqlx::Result<()> {
    let now = now_ms();
    sqlx::query(
        "INSERT INTO user_agent_profiles (workspace_id, user_id, skipped_at, created_at, updated_at) VALUES (?1, ?2, ?3, ?3, ?3)
         ON CONFLICT(workspace_id, user_id) DO UPDATE SET skipped_at = excluded.skipped_at",
    )
    .bind(ws)
    .bind(user_id)
    .bind(now)
    .execute(db)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------- who the agent works for

/// The person whose mailbox an email arrived in, with their profile if they wrote one.
#[derive(Debug, Clone)]
pub struct Owner {
    pub user_id: String,
    pub email: String,
    pub profile: Option<UserProfile>,
}

impl Owner {
    pub fn routine_ok(&self) -> bool {
        self.profile.as_ref().is_none_or(UserProfile::routine_ok)
    }
}

/// Everything the agent needs to know about who it is and who it works for on one email.
#[derive(Debug, Clone)]
pub struct AgentContext {
    pub agent: WorkspaceAgent,
    pub mailbox: Option<String>,
    pub owner: Option<Owner>,
}

impl AgentContext {
    /// The trusted prompt blocks (agent persona + the owner's profile) for planner and drafter.
    pub fn blocks(&self) -> crate::agents::persona::PromptBlocks {
        crate::agents::persona::build_prompt_blocks(&self.agent, self.mailbox.as_deref(), self.owner.as_ref(), None)
    }

    /// Trusted block for the planner, placed after the safety rules.
    pub fn planner_block(&self) -> String {
        self.blocks().planner
    }

    /// How drafts for this email should sound: the agent's persona plus the owner's profile,
    /// the owner's signature (never the agent's own) and the quality tier's model.
    pub fn draft_style(&self, config: &Config) -> DraftStyle {
        let signature = self.owner.as_ref().and_then(|o| o.profile.as_ref()).and_then(|p| p.tone.sign_off.clone());
        let model = Some(tier_model(config, effective_tier(&self.agent.quality, false)).to_string()).filter(|m| !m.is_empty());
        let premium_model = Some(tier_model(config, effective_tier(&self.agent.quality, true)).to_string()).filter(|m| !m.is_empty());
        DraftStyle { instructions: Some(self.blocks().drafter).filter(|i| !i.trim().is_empty()), signature, model, premium_model }
    }
}

#[derive(sqlx::FromRow)]
struct MailboxRow {
    mailbox: Option<String>,
    owner_id: Option<String>,
    owner_email: Option<String>,
}

/// The agent, the mailbox and its owner for one email of this workspace.
pub async fn context_for_message(db: &SqlitePool, config: &Config, ws: &str, message_id: &str) -> sqlx::Result<AgentContext> {
    let agent = agent(db, config, ws).await?;
    let row: Option<MailboxRow> = sqlx::query_as(
        "SELECT COALESCE(s.mailbox_address, s.inbound_alias) AS mailbox, u.id AS owner_id, u.email AS owner_email
         FROM email_messages m JOIN email_sources s ON s.id = m.source_id
         LEFT JOIN users u ON u.id = s.connected_by_user_id
           AND EXISTS (SELECT 1 FROM memberships ms WHERE ms.workspace_id = m.workspace_id AND ms.user_id = u.id)
         WHERE m.id = ?1 AND m.workspace_id = ?2",
    )
    .bind(message_id)
    .bind(ws)
    .fetch_optional(db)
    .await?;
    let Some(row) = row else { return Ok(AgentContext { agent, mailbox: None, owner: None }) };
    let owner = match (row.owner_id, row.owner_email) {
        (Some(user_id), Some(email)) => Some(Owner { profile: user_profile(db, ws, &user_id).await?, user_id, email }),
        _ => None,
    };
    Ok(AgentContext { agent, mailbox: row.mailbox.filter(|m| !m.trim().is_empty()), owner })
}

/// The agent and the person it works for on a case (from the case's email).
pub async fn context_for_case(db: &SqlitePool, config: &Config, case: &crate::agents::case::Case) -> sqlx::Result<AgentContext> {
    match case.message_id.as_deref() {
        Some(m) => context_for_message(db, config, &case.workspace_id, m).await,
        None => Ok(AgentContext { agent: agent(db, config, &case.workspace_id).await?, mailbox: None, owner: None }),
    }
}

// ---------------------------------------------------------------- drafting style

/// Overrides for the drafter while the agent writes. `None` fields keep the workspace values.
#[derive(Debug, Clone, Default)]
pub struct DraftStyle {
    pub instructions: Option<String>,
    pub signature: Option<String>,
    /// The model for the agent's own tier; internal only.
    pub model: Option<String>,
    /// The model for a premium call (the agent's tier raised to `best`); internal only.
    pub premium_model: Option<String>,
}

tokio::task_local! {
    static DRAFT_STYLE: DraftStyle;
}

/// Runs `f` (a drafting call) with the agent's style.
pub async fn with_draft_style<F: Future>(style: DraftStyle, f: F) -> F::Output {
    DRAFT_STYLE.scope(style, f).await
}

/// Hook for the drafter: how a reply to this email should sound. A style set for this call
/// (agent cases, previews) wins; otherwise the workspace agent and the mailbox owner's profile.
pub async fn draft_voice(
    db: &SqlitePool,
    config: &Config,
    ws: &str,
    message_id: &str,
    instructions: String,
    signature: Option<String>,
) -> sqlx::Result<(String, Option<String>)> {
    let style = match DRAFT_STYLE.try_with(Clone::clone) {
        Ok(style) => style,
        Err(_) => context_for_message(db, config, ws, message_id).await?.draft_style(config),
    };
    Ok(merge_voice(style, instructions, signature))
}

/// The agent's voice replaces the standard guidance; the workspace's own reply instructions are
/// kept when the persona doesn't already include them. Signature: the style's, else the workspace's.
pub fn merge_voice(style: DraftStyle, workspace_instructions: String, signature: Option<String>) -> (String, Option<String>) {
    let signature = style.signature.or(signature);
    let Some(persona) = style.instructions else { return (workspace_instructions, signature) };
    let team = workspace_instructions.trim();
    if team.is_empty() || team == DEFAULT_REPLY_STYLE || team == crate::packs::NEUTRAL_REPLY_STYLE || persona.contains(team) {
        return (persona, signature);
    }
    (format!("{persona}\n\nTeam reply instructions:\n{team}"), signature)
}

/// Hook for the drafting model: the agent's tier model when one is set for this call. A premium
/// draft uses the `best` tier's model if one is configured, else the drafter's own premium model.
pub fn draft_model_or(model: &str, premium: bool) -> String {
    DRAFT_STYLE
        .try_with(|s| if premium { s.premium_model.clone() } else { s.model.clone() })
        .ok()
        .flatten()
        .unwrap_or_else(|| model.to_string())
}

// ---------------------------------------------------------------- validation

fn some_value<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

fn parse_list(json: &str, allowed: &[&str]) -> Vec<String> {
    let given: Vec<String> = serde_json::from_str(json).unwrap_or_default();
    allowed.iter().filter(|a| given.iter().any(|g| g == *a)).map(|a| a.to_string()).collect()
}

fn pick_list(given: &[String], allowed: &[&str], what: &str) -> Result<Vec<String>, String> {
    if let Some(bad) = given.iter().find(|g| !allowed.contains(&g.as_str())) {
        return Err(format!("unknown {what} {}; use {}", crate::agents::sentence(bad, 30), allowed.join(", ")));
    }
    Ok(allowed.iter().filter(|a| given.iter().any(|g| g == *a)).map(|a| a.to_string()).collect())
}

fn bounded(value: &str, max: usize, what: &str) -> Result<String, String> {
    let v = value.trim();
    if v.chars().count() > max {
        return Err(format!("{what} can be at most {max} characters"));
    }
    Ok(v.to_string())
}

fn signature_of(value: &Option<String>) -> Result<Option<String>, String> {
    let s = value.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if s.is_some_and(|s| s.chars().count() > MAX_SIGNATURE_CHARS) {
        return Err(format!("the signature can be at most {MAX_SIGNATURE_CHARS} characters"));
    }
    Ok(s.map(str::to_string))
}

/// A partial update of the agent. Absent fields keep their value; `signature: null` clears it.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentInput {
    pub name: Option<String>,
    pub avatar: Option<String>,
    pub color: Option<String>,
    pub persona: Option<String>,
    #[serde(default, deserialize_with = "some_value")]
    pub signature: Option<Option<String>>,
    pub tasks: Option<Vec<String>>,
    pub permissions: Option<PermissionsInput>,
    pub quality: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionsInput {
    pub draft: Option<bool>,
    pub send: Option<String>,
    /// Accepted only as "ask": submissions always need a person.
    pub submit_candidate: Option<String>,
}

/// The editable fields of the agent, after validation.
pub type AgentFields = WorkspaceAgent;

impl WorkspaceAgent {
    /// Applies an update on top of this agent, validating every given field.
    pub fn apply(mut self, input: &AgentInput) -> Result<Self, String> {
        if let Some(name) = &input.name {
            self.name = name.split_whitespace().collect::<Vec<_>>().join(" ");
            let n = self.name.chars().count();
            if n == 0 || n > MAX_NAME_CHARS {
                return Err(format!("the name must be 1 to {MAX_NAME_CHARS} characters"));
            }
        }
        if let Some(avatar) = &input.avatar {
            self.avatar = valid_avatar(avatar).ok_or("the avatar must be a picture, one emoji or 1 to 2 letters")?;
        }
        if let Some(color) = &input.color {
            if !COLORS.contains(&color.as_str()) {
                return Err(format!("color must be one of {}", COLORS.join(", ")));
            }
            self.color = color.clone();
        }
        if let Some(persona) = &input.persona {
            self.persona = bounded(persona, MAX_PERSONA_CHARS, "the instructions")?;
        }
        if let Some(signature) = &input.signature {
            self.signature = signature_of(signature)?;
        }
        if let Some(tasks) = &input.tasks {
            self.tasks = pick_list(tasks, TASKS, "job")?;
        }
        if let Some(p) = &input.permissions {
            if let Some(draft) = p.draft {
                self.permissions.draft = draft;
            }
            if let Some(send) = &p.send {
                if !SEND_MODES.contains(&send.as_str()) {
                    return Err("send must be never, ask or auto".into());
                }
                self.permissions.send = send.clone();
            }
            if p.submit_candidate.as_deref().is_some_and(|s| s != "ask") {
                return Err("submitting candidates always needs a person's approval".into());
            }
        }
        if let Some(quality) = &input.quality {
            if !QUALITIES.contains(&quality.as_str()) {
                return Err("quality must be fast, balanced or best".into());
            }
            self.quality = quality.clone();
        }
        if let Some(enabled) = input.enabled {
            self.enabled = enabled;
        }
        Ok(self)
    }
}

/// A person's profile, as they fill it in. Absent fields keep their value.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserProfileInput {
    pub display_name: Option<String>,
    pub job_title: Option<String>,
    pub handles: Option<Vec<String>>,
    pub handles_note: Option<String>,
    pub ignore_note: Option<String>,
    pub audiences: Option<Vec<String>>,
    pub vips: Option<Vec<String>>,
    pub tone: Option<ToneInput>,
    pub working_hours: Option<String>,
    pub timezone: Option<String>,
    pub never_do: Option<String>,
    pub approvals: Option<ApprovalsInput>,
}

/// `sign_off: null` clears the signature.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToneInput {
    pub formality: Option<String>,
    pub length: Option<String>,
    #[serde(default, deserialize_with = "some_value")]
    pub sign_off: Option<Option<String>>,
    pub language: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalsInput {
    pub send_email: Option<String>,
    /// Accepted only as "always_ask": submissions always need a person.
    pub submit_candidate: Option<String>,
}

fn one_of(value: &str, allowed: &[&str], what: &str) -> Result<String, String> {
    let v = value.trim();
    if !v.is_empty() && !allowed.contains(&v) {
        return Err(format!("{what} must be one of {}", allowed.join(", ")));
    }
    Ok(v.to_string())
}

/// An address or a domain (lower-case, no spaces).
fn valid_vip(v: &str) -> Option<String> {
    let v = v.trim().trim_start_matches('@').to_lowercase();
    let ok = !v.is_empty() && v.len() <= 100 && v.contains('.') && !v.chars().any(char::is_whitespace) && v.matches('@').count() <= 1;
    ok.then_some(v)
}

impl UserProfile {
    /// Applies an update on top of this profile, validating every given field.
    pub fn apply(mut self, i: &UserProfileInput) -> Result<Self, String> {
        let text = |v: &Option<String>, current: &str, max: usize, what: &str| match v {
            Some(v) => bounded(v, max, what),
            None => Ok(current.to_string()),
        };
        self.display_name = text(&i.display_name, &self.display_name, MAX_NAME_CHARS, "your name")?;
        self.job_title = text(&i.job_title, &self.job_title, 80, "the job title")?;
        self.handles_note = text(&i.handles_note, &self.handles_note, 500, "what you handle")?;
        self.ignore_note = text(&i.ignore_note, &self.ignore_note, 500, "what to leave alone")?;
        self.working_hours = text(&i.working_hours, &self.working_hours, 80, "working hours")?;
        self.timezone = text(&i.timezone, &self.timezone, 60, "the time zone")?;
        self.never_do = text(&i.never_do, &self.never_do, 1_000, "things to never do")?;
        if let Some(h) = &i.handles {
            self.handles = pick_list(h, HANDLES, "topic")?;
        }
        if let Some(a) = &i.audiences {
            self.audiences = pick_list(a, AUDIENCES, "audience")?;
        }
        if let Some(vips) = &i.vips {
            let mut out: Vec<String> = Vec::new();
            for v in vips.iter().filter(|v| !v.trim().is_empty()) {
                let v = valid_vip(v).ok_or_else(|| format!("{} is not an email address or domain", crate::agents::sentence(v, 40)))?;
                if !out.contains(&v) {
                    out.push(v);
                }
            }
            if out.len() > MAX_VIPS {
                return Err(format!("list at most {MAX_VIPS} important people or domains"));
            }
            self.vips = out;
        }
        if let Some(t) = &i.tone {
            if let Some(f) = &t.formality {
                self.tone.formality = one_of(f, FORMALITIES, "formality")?;
            }
            if let Some(l) = &t.length {
                self.tone.length = one_of(l, LENGTHS, "length")?;
            }
            if let Some(lang) = &t.language {
                self.tone.language = bounded(lang, 40, "the language")?;
            }
            if let Some(sign_off) = &t.sign_off {
                self.tone.sign_off = signature_of(sign_off)?;
            }
        }
        if let Some(a) = &i.approvals {
            if let Some(send) = &a.send_email {
                if !SEND_APPROVALS.contains(&send.as_str()) {
                    return Err("send approval must be always_ask or routine_ok".into());
                }
                self.approvals.send_email = send.clone();
            }
            if a.submit_candidate.as_deref().is_some_and(|s| s != "always_ask") {
                return Err("submitting candidates always needs a person's approval".into());
            }
        }
        Ok(self)
    }
}

/// The picture avatars the web app ships in `/avatars`, stored as `fluent:<key>`.
const PICTURE_AVATARS: &[&str] = &[
    "blossom", "owl", "bee", "dolphin", "fox", "panda", "koala", "robot", "rocket", "sparkles", "penguin", "otter", "hedgehog", "rabbit", "cat", "turtle",
];

/// A picture avatar (`fluent:owl`), one emoji, or 1–2 letters/digits (upper-cased).
pub fn valid_avatar(avatar: &str) -> Option<String> {
    let a = avatar.trim();
    if let Some(key) = a.strip_prefix("fluent:") {
        return PICTURE_AVATARS.contains(&key).then(|| a.to_string());
    }
    let n = a.chars().count();
    if n == 0 {
        return None;
    }
    if a.chars().all(char::is_alphanumeric) {
        return (n <= 2).then(|| a.to_uppercase());
    }
    (n <= 8 && a.chars().all(|c| !c.is_ascii() && !c.is_whitespace() && !c.is_alphanumeric())).then(|| a.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent() -> WorkspaceAgent {
        WorkspaceAgent {
            workspace_id: "ws".into(),
            name: DEFAULT_NAME.into(),
            avatar: DEFAULT_AVATAR.into(),
            color: DEFAULT_COLOR.into(),
            persona: "Warm.".into(),
            signature: Some("Meera, Support Desk".into()),
            tasks: vec!["reply".into(), "requirements".into()],
            permissions: Permissions::default(),
            quality: "balanced".into(),
            enabled: true,
            updated_at: 0,
        }
    }

    fn input<T: for<'de> Deserialize<'de>>(v: Value) -> T {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn avatars_are_one_emoji_or_two_letters() {
        assert_eq!(valid_avatar(" as "), Some("AS".into()));
        assert_eq!(valid_avatar("🌼"), Some("🌼".into()));
        assert_eq!(valid_avatar("👩‍💼"), Some("👩‍💼".into()));
        assert_eq!(valid_avatar("fluent:owl"), Some("fluent:owl".into()));
        assert_eq!(valid_avatar("fluent:nope"), None);
        assert_eq!(valid_avatar("abc"), None);
        assert_eq!(valid_avatar("a!"), None);
        assert_eq!(valid_avatar(""), None);
    }

    #[test]
    fn validates_agent_updates() {
        let a = agent()
            .apply(&input(json!({"name": "  Asha   ", "tasks": ["requirements", "reply"], "permissions": {"send": "auto"}, "signature": null})))
            .unwrap();
        assert_eq!((a.name.as_str(), a.signature.as_deref(), a.permissions.send.as_str()), ("Asha", None, "auto"));
        assert_eq!(a.tasks, ["reply", "requirements"]);
        assert_eq!(agent().apply(&input(json!({}))).unwrap(), agent(), "absent fields are kept");
        let bad = |v: Value| agent().apply(&input(v)).is_err();
        assert!(bad(json!({"name": ""})));
        assert!(bad(json!({"name": "x".repeat(61)})));
        assert!(bad(json!({"persona": "x".repeat(3001)})));
        assert!(bad(json!({"tasks": ["payroll"]})));
        assert!(bad(json!({"color": "pink"})));
        assert!(bad(json!({"quality": "premium"})));
        assert!(bad(json!({"permissions": {"send": "sometimes"}})));
        assert!(bad(json!({"permissions": {"submit_candidate": "auto"}})));
        assert!(serde_json::from_value::<AgentInput>(json!({"model": "x"})).is_err());
    }

    #[test]
    fn validates_user_profiles() {
        let p = UserProfile::blank()
            .apply(&input(json!({"display_name": "Priya", "job_title": "Recruiter", "handles": ["candidates", "leave"],
                                 "audiences": ["clients"], "vips": ["CEO@Client.com", "@client.com", "client.com"],
                                 "tone": {"formality": "friendly", "length": "short", "sign_off": " Priya ", "language": "English"},
                                 "approvals": {"send_email": "routine_ok"}})))
            .unwrap();
        assert_eq!(p.handles, ["candidates", "leave"]);
        assert_eq!(p.vips, ["ceo@client.com", "client.com"]);
        assert_eq!((p.tone.formality.as_str(), p.tone.sign_off.as_deref()), ("friendly", Some("Priya")));
        assert!(p.routine_ok(), "an unsaved profile has not asked to be asked first");
        assert!(UserProfile { saved: true, ..p.clone() }.routine_ok());
        assert!(!UserProfile { saved: true, approvals: ProfileApprovals { send_email: "always_ask".into() }, ..p.clone() }.routine_ok());
        let cleared = p.apply(&input(json!({"tone": {"sign_off": null}}))).unwrap();
        assert_eq!(cleared.tone.sign_off, None);
        let bad = |v: Value| UserProfile::blank().apply(&input(v)).is_err();
        assert!(bad(json!({"handles": ["hacking"]})));
        assert!(bad(json!({"tone": {"formality": "angry"}})));
        assert!(bad(json!({"tone": {"length": "epic"}})));
        assert!(bad(json!({"approvals": {"send_email": "never"}})));
        assert!(bad(json!({"approvals": {"submit_candidate": "routine_ok"}})));
        assert!(bad(json!({"vips": ["not an address"]})));
        assert!(bad(json!({"vips": (0..21).map(|i| format!("p{i}@x.com")).collect::<Vec<_>>()})));
        assert!(bad(json!({"never_do": "x".repeat(1001)})));
    }

    #[test]
    fn tiers_combine_with_one_rule() {
        for (agent, premium, want) in [
            ("fast", false, "fast"), ("fast", true, "best"), ("balanced", false, "balanced"), ("balanced", true, "best"),
            ("best", false, "best"), ("best", true, "best"), ("anything else", false, "balanced"),
        ] {
            assert_eq!(effective_tier(agent, premium), want, "{agent} premium={premium}");
        }
        let mut config = Config::for_tests("sqlite::memory:");
        config.agent_fast_model_id = "fast-m".into();
        config.agent_best_model_id = "best-m".into();
        let ctx = |quality: &str| AgentContext {
            agent: WorkspaceAgent { quality: quality.into(), ..agent() },
            mailbox: None,
            owner: None,
        };
        let style = ctx("fast").draft_style(&config);
        assert_eq!((style.model.as_deref(), style.premium_model.as_deref()), (Some("fast-m"), Some("best-m")), "premium raises the floor");
        let style = ctx("balanced").draft_style(&config);
        assert_eq!((style.model.as_deref(), style.premium_model.as_deref()), (None, Some("best-m")), "no balanced model configured");
        let style = ctx("best").draft_style(&config);
        assert_eq!((style.model.as_deref(), style.premium_model.as_deref()), (Some("best-m"), Some("best-m")));
    }

    #[test]
    fn stored_permissions_fall_back_to_asking() {
        assert_eq!(Permissions::parse("garbage"), Permissions::default());
        assert_eq!(Permissions::parse(r#"{"draft":false,"send":"auto"}"#), Permissions { draft: false, send: "auto".into() });
        assert_eq!(Permissions::parse(r#"{"send":"yolo"}"#).send, "ask");
    }

    #[test]
    fn seeded_persona_is_friendly_and_adds_workspace_instructions() {
        assert!(standard_guidance(DEFAULT_REPLY_STYLE).starts_with("I'm Meera") && standard_guidance(DEFAULT_REPLY_STYLE).contains(DEFAULT_REPLY_STYLE));
        assert!(default_persona(DEFAULT_REPLY_STYLE, "Mention our 1-day turnaround.").ends_with("\n\nMention our 1-day turnaround."));
    }

    #[test]
    fn context_blocks_name_the_agent_and_the_person() {
        let profile = UserProfile { saved: true, ..UserProfile::blank() }
            .apply(&input(json!({"display_name": "Priya", "job_title": "Recruiter", "never_do": "Share salary bands", "tone": {"sign_off": "Priya S."}})))
            .unwrap();
        let ctx = AgentContext {
            agent: agent(),
            mailbox: Some("priya@acme.com".into()),
            owner: Some(Owner { user_id: "u1".into(), email: "priya@acme.com".into(), profile: Some(profile) }),
        };
        let block = ctx.planner_block();
        assert!(block.contains("You are Meera, the team's email agent for the mailbox priya@acme.com."));
        assert!(block.contains("Warm.") && block.contains("You are working for Priya (priya@acme.com), Recruiter."));
        assert!(block.contains("- Never do this for them: Share salary bands"));
        let style = ctx.draft_style(&Config::for_tests("sqlite::memory:"));
        assert_eq!(style.signature.as_deref(), Some("Priya S."), "the person's signature wins");
        assert!(style.instructions.unwrap().contains("Recruiter"));
        assert_eq!(style.model, None, "no tier model configured in tests");
    }

    #[tokio::test]
    async fn draft_style_applies_only_inside_scope() {
        assert_eq!(draft_model_or("m", false), "m");
        let style = DraftStyle { instructions: Some("persona".into()), signature: None, model: Some("tier".into()), premium_model: Some("top".into()) };
        let inside = with_draft_style(style.clone(), async { (draft_model_or("m", false), draft_model_or("m", true)) }).await;
        assert_eq!(inside, ("tier".to_string(), "top".to_string()));
        let no_premium = DraftStyle { premium_model: None, ..style };
        assert_eq!(with_draft_style(no_premium, async { draft_model_or("own premium", true) }).await, "own premium");
        let none = DraftStyle::default();
        assert_eq!(merge_voice(none, "ws".into(), Some("sig".into())), ("ws".to_string(), Some("sig".to_string())));
        let persona = || DraftStyle { instructions: Some("Be Meera.".into()), signature: Some("Meera".into()), model: None, premium_model: None };
        assert_eq!(merge_voice(persona(), DEFAULT_REPLY_STYLE.into(), None), ("Be Meera.".to_string(), Some("Meera".to_string())));
        assert_eq!(merge_voice(persona(), "Mention SLAs.".into(), None).0, "Be Meera.\n\nTeam reply instructions:\nMention SLAs.");
    }
}
