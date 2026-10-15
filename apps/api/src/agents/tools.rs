//! Agent tools: each is a thin, schema-described wrapper over an existing function.
//! `read`/`low` tools run at once (after the critic for `low`); `high` tools only run when a
//! person approves them.

use std::sync::LazyLock;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Value, json};

use super::{case::Case, sentence};
use crate::{
    ai::DraftQuality,
    audit::{self, Actor, Entry},
    db::{iso, new_id, now_ms},
    error::AppError,
    http::AppState,
    jobs::handlers::{self, JobError},
    knowledge, memory, replies,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    Read,
    Low,
    High,
}

impl Risk {
    pub fn as_str(self) -> &'static str {
        match self {
            Risk::Read => "read",
            Risk::Low => "low",
            Risk::High => "high",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// A plain sentence the planner (and a recruiter) can read.
    #[error("{0}")]
    Failed(String),
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
}

impl From<JobError> for ToolError {
    fn from(e: JobError) -> Self {
        match e {
            JobError::Retry(m) if m.starts_with("database") => ToolError::Failed("A temporary storage problem stopped this step.".into()),
            JobError::Retry(_) => ToolError::Failed("The AI service did not answer; try again later.".into()),
            JobError::Permanent(m) if m == "message not found" => ToolError::Failed("That email was not found.".into()),
            JobError::Permanent(_) => ToolError::Failed("The AI service could not complete this step.".into()),
        }
    }
}

impl From<AppError> for ToolError {
    fn from(e: AppError) -> Self {
        match e {
            AppError::DraftIncomplete(m) | AppError::Conflict(m) | AppError::BadRequest(m) => ToolError::Failed(m),
            AppError::NotFound => ToolError::Failed("That item was not found.".into()),
            AppError::Unavailable => ToolError::Failed("The mailbox could not be reached; try again later.".into()),
            _ => ToolError::Failed("The action could not be completed.".into()),
        }
    }
}

fn failed<T>(msg: &str) -> Result<T, ToolError> {
    Err(ToolError::Failed(msg.into()))
}

pub struct ToolCtx<'a> {
    pub state: &'a AppState,
    pub case: &'a Case,
    pub turn: i64,
    /// Who the action is done for: the system, or the person who approved it.
    pub actor: Actor<'a>,
    /// An automatic send is held this many seconds (0 for a send a person approved).
    pub hold_secs: i64,
}

impl ToolCtx<'_> {
    fn ws(&self) -> &str {
        &self.case.workspace_id
    }
}

#[derive(Debug, Clone, Default)]
pub struct ToolOutput {
    pub value: Value,
    /// One plain sentence for the timeline.
    pub summary: String,
    /// Facts to merge into the case state.
    pub state: Option<Value>,
    /// The case sleeps until this time (unix ms).
    pub sleep_until: Option<i64>,
    /// What the tool itself spent on models (reading an email, say); added to the case's spend.
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Pick {
    pub id: String,
    pub name: String,
    pub score: i64,
}

/// What a `high` tool would do, shown to the critic and the approver.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Preview {
    pub summary: String,
    pub to: Option<String>,
    pub subject: Option<String>,
    pub body: String,
    pub draft_id: Option<String>,
    pub draft_status: Option<String>,
    pub thread_id: Option<String>,
    pub requirement_id: Option<String>,
    pub candidates: Vec<Pick>,
    /// Requested candidate ids that are not matches of the requirement.
    pub unmatched_candidates: Vec<String>,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    /// JSON Schema (object) for the arguments.
    fn schema(&self) -> Value;
    fn risk(&self) -> Risk;
    /// For `high` tools: what would happen, without doing it.
    async fn preview(&self, _ctx: &ToolCtx<'_>, _args: &Value) -> Result<Option<Preview>, ToolError> {
        Ok(None)
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError>;
}

pub struct Registry {
    tools: Vec<Box<dyn Tool>>,
}

impl Registry {
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.iter().find(|t| t.name() == name).map(|t| t.as_ref())
    }

    pub fn all(&self) -> impl Iterator<Item = &dyn Tool> {
        self.tools.iter().map(|t| t.as_ref())
    }

    /// OpenAI-style function specs for the planner, with `finish` last.
    pub fn specs(&self) -> Vec<Value> {
        self.specs_for(&self.all().map(|t| t.name().to_string()).collect::<Vec<_>>())
    }

    /// The specs of only these tools (an agent's allowlist), with `finish` last.
    pub fn specs_for(&self, allowed: &[String]) -> Vec<Value> {
        let mut specs: Vec<Value> = self
            .all()
            .filter(|t| allowed.iter().any(|a| a == t.name()))
            .map(|t| {
                json!({"type": "function", "function": {
                    "name": t.name(),
                    "description": format!("{} (risk: {})", t.description(), t.risk().as_str()),
                    "parameters": t.schema()
                }})
            })
            .collect();
        specs.push(super::planner::finish_spec());
        specs
    }
}

static REGISTRY: LazyLock<Registry> = LazyLock::new(|| Registry {
    tools: vec![
        Box::new(SearchMail),
        Box::new(ReadEmail),
        Box::new(ReadThread),
        Box::new(FindCandidates),
        Box::new(FindRequirements),
        Box::new(RecallMemory),
        Box::new(DraftEmail),
        Box::new(CreateTask),
        Box::new(ScheduleCheck),
        Box::new(UpdateCase),
        Box::new(ExtractFields),
        Box::new(RecordTimesheet),
        Box::new(LabelEmail),
        Box::new(SaveToGmailDrafts),
        Box::new(SendEmail),
        Box::new(SubmitCandidate),
        Box::new(ProposeAutomation),
        Box::new(SendMessage),
        Box::new(NotifyOwner),
    ],
});

pub fn registry() -> &'static Registry {
    &REGISTRY
}

/// Approvals name what a person decides; a tool's name can differ.
pub fn approval_action(tool: &str) -> &str {
    if tool == "propose_automation" { "create_automation" } else { tool }
}

/// The tool that applies an approved action.
pub fn tool_for_approval(action: &str) -> &str {
    if action == "create_automation" { "propose_automation" } else { action }
}

/// Checks arguments against a tool schema: an object, required keys present, known keys typed.
pub fn validate_args(schema: &Value, args: &Value) -> Result<(), String> {
    let Some(obj) = args.as_object() else { return Err("arguments must be an object".into()) };
    for key in schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        if obj.get(key).is_none_or(Value::is_null) {
            return Err(format!("missing required field {key}"));
        }
    }
    let props = schema.get("properties").and_then(Value::as_object);
    for (key, value) in obj {
        let Some(prop) = props.and_then(|p| p.get(key)) else { continue };
        if value.is_null() {
            continue;
        }
        let ok = match prop.get("type").and_then(Value::as_str) {
            Some("string") => value.as_str().is_some_and(|s| {
                prop.get("enum").and_then(Value::as_array).is_none_or(|e| e.iter().any(|x| x.as_str() == Some(s)))
            }),
            Some("integer") => value.as_i64().is_some(),
            Some("number") => value.as_f64().is_some(),
            Some("boolean") => value.is_boolean(),
            Some("array") => value.as_array().is_some_and(|items| {
                prop.pointer("/items/type").and_then(Value::as_str) != Some("string") || items.iter().all(Value::is_string)
            }),
            _ => true,
        };
        if !ok {
            return Err(format!("field {key} has the wrong type or value"));
        }
    }
    Ok(())
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ToolError::Failed(format!("{key} is required")))
}

fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// Words of an email the planner sees by default; `read_email {excerpt: true}` asks for a longer one.
const EXCERPT_WORDS: usize = 45;
const LONG_EXCERPT_WORDS: usize = 200;
const THREAD_EXCERPT_WORDS: usize = 40;

/// Email text for the planner: cleaned, cut short, words joined by `^` and fenced, so it reads as
/// data. The planner never sees a raw body.
fn fenced(text: &str, words: usize) -> String {
    format!("<<{}>>", crate::assistant::datamark(&knowledge::clean_body(text), words))
}

fn limit_arg(args: &Value, default: i64, max: i64) -> i64 {
    args.get("limit").and_then(Value::as_i64).unwrap_or(default).clamp(1, max)
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    id: String,
    subject: String,
    sender_address: String,
    body_text: String,
    received_at: i64,
    direction: String,
    thread_id: String,
}

async fn message_in_ws(ctx: &ToolCtx<'_>, message_id: &str) -> Result<MessageRow, ToolError> {
    sqlx::query_as(
        "SELECT id, subject, sender_address, body_text, received_at, direction, thread_id
         FROM email_messages WHERE id = ?1 AND workspace_id = ?2 AND suppressed = 0",
    )
    .bind(message_id)
    .bind(ctx.ws())
    .fetch_optional(&ctx.state.db)
    .await?
    .ok_or_else(|| ToolError::Failed("That email was not found in this workspace.".into()))
}

// ---------------------------------------------------------------- read tools

struct SearchMail;

#[async_trait]
impl Tool for SearchMail {
    fn name(&self) -> &'static str {
        "search_mail"
    }
    fn description(&self) -> &'static str {
        "Search this workspace's emails and attachments by meaning and keywords."
    }
    fn schema(&self) -> Value {
        object(json!({"query": {"type": "string"}, "limit": {"type": "integer", "description": "1 to 10, default 5"}}), &["query"])
    }
    fn risk(&self) -> Risk {
        Risk::Read
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let query = str_arg(args, "query")?;
        let hits = knowledge::hybrid_search(
            &ctx.state.db,
            ctx.state.embedder.as_ref(),
            ctx.ws(),
            query,
            knowledge::Scope::Documents,
            limit_arg(args, 5, 10) as usize,
        )
        .await?;
        let items: Vec<Value> = hits
            .iter()
            .map(|h| json!({"message_id": h.message_id, "kind": h.kind, "snippet": fenced(&knowledge::snippet(knowledge::chunk_body(&h.text), query, 240), THREAD_EXCERPT_WORDS)}))
            .collect();
        Ok(ToolOutput {
            summary: format!("Searched mail for \"{}\" and found {} result{}", sentence(query, 60), items.len(), plural(items.len())),
            value: json!({"items": items}),
            ..Default::default()
        })
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

struct ReadEmail;

#[async_trait]
impl Tool for ReadEmail {
    fn name(&self) -> &'static str {
        "read_email"
    }
    fn description(&self) -> &'static str {
        "Read one email: what it asks (summary, asks, deadline) and a short fenced excerpt. Pass excerpt: true for a longer one."
    }
    fn schema(&self) -> Value {
        object(json!({"message_id": {"type": "string"}, "excerpt": {"type": "boolean"}}), &["message_id"])
    }
    fn risk(&self) -> Risk {
        Risk::Read
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let msg = message_in_ws(ctx, str_arg(args, "message_id")?).await?;
        let insight = if msg.direction == "inbound" {
            Some(handlers::analyze_message(ctx.state, &json!({"message_id": msg.id})).await?)
        } else {
            None
        };
        Ok(ToolOutput {
            summary: format!("Read the email \"{}\" from {}", sentence(&msg.subject, 80), msg.sender_address),
            value: json!({
                "message_id": msg.id, "from": msg.sender_address, "subject": msg.subject, "received_at": iso(msg.received_at),
                "direction": msg.direction,
                "excerpt": fenced(&msg.body_text, if args.get("excerpt").and_then(Value::as_bool) == Some(true) { LONG_EXCERPT_WORDS } else { EXCERPT_WORDS }),
                "insight": insight,
            }),
            ..Default::default()
        })
    }
}

struct ReadThread;

#[async_trait]
impl Tool for ReadThread {
    fn name(&self) -> &'static str {
        "read_thread"
    }
    fn description(&self) -> &'static str {
        "Read the whole conversation an email belongs to, oldest first, including replies we sent."
    }
    fn schema(&self) -> Value {
        object(json!({"message_id": {"type": "string"}}), &["message_id"])
    }
    fn risk(&self) -> Risk {
        Risk::Read
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let msg = message_in_ws(ctx, str_arg(args, "message_id")?).await?;
        let rows: Vec<MessageRow> = sqlx::query_as(
            "SELECT id, subject, sender_address, body_text, received_at, direction, thread_id
             FROM email_messages WHERE thread_id = ?1 AND workspace_id = ?2 AND suppressed = 0 ORDER BY received_at, id LIMIT 30",
        )
        .bind(&msg.thread_id)
        .bind(ctx.ws())
        .fetch_all(&ctx.state.db)
        .await?;
        let sent: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT d.id, d.body_text, d.updated_at FROM drafts d JOIN email_messages m ON m.id = d.message_id
             WHERE m.thread_id = ?1 AND d.workspace_id = ?2 AND d.status = 'sent' ORDER BY d.updated_at LIMIT 20",
        )
        .bind(&msg.thread_id)
        .bind(ctx.ws())
        .fetch_all(&ctx.state.db)
        .await?;
        let mut entries: Vec<(i64, Value)> = rows
            .iter()
            .map(|m| {
                (m.received_at, json!({
                    "message_id": m.id, "from": m.sender_address, "direction": m.direction, "subject": m.subject,
                    "received_at": iso(m.received_at),
                    "excerpt": fenced(&m.body_text, THREAD_EXCERPT_WORDS),
                }))
            })
            .collect();
        entries.extend(sent.into_iter().map(|(id, body, at)| {
            (at, json!({"draft_id": id, "from": "us", "direction": "sent_reply", "received_at": iso(at),
                        "excerpt": fenced(&body, THREAD_EXCERPT_WORDS)}))
        }));
        entries.sort_by_key(|(at, _)| *at);
        let inbound = rows.iter().filter(|m| m.direction == "inbound").count();
        Ok(ToolOutput {
            summary: format!("Read the conversation ({} email{}, {} from the other side)", entries.len(), plural(entries.len()), inbound),
            value: json!({"thread_id": msg.thread_id, "messages": entries.into_iter().map(|(_, v)| v).collect::<Vec<_>>()}),
            ..Default::default()
        })
    }
}

struct FindCandidates;

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

struct FindRequirements;

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

struct RecallMemory;

#[async_trait]
impl Tool for RecallMemory {
    fn name(&self) -> &'static str {
        "recall_memory"
    }
    fn description(&self) -> &'static str {
        "What the team knows about a contact (email address) or a topic/client name."
    }
    fn schema(&self) -> Value {
        object(json!({"about": {"type": "string", "description": "An email address, client or topic"}}), &["about"])
    }
    fn risk(&self) -> Risk {
        Risk::Read
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let about = str_arg(args, "about")?.to_lowercase();
        let agent = {
            let mut conn = ctx.state.db.acquire().await?;
            crate::learning::scope::resolve(&mut conn, ctx.ws(), ctx.case.agent_id.as_deref(), None).await?
        };
        let view = agent.agent_id.clone().map(|agent_id| memory::AgentMemory { agent_id, scope: agent.memory });
        let (sender, topic) = if about.contains('@') { (about.as_str(), "") } else { ("", about.as_str()) };
        let mem = memory::context_for_agent(&ctx.state.db, ctx.ws(), sender, topic, false, view.as_ref()).await?;
        let notes = mem.lines();
        Ok(ToolOutput {
            summary: format!("Recalled {} note{} about {}", notes.len(), plural(notes.len()), sentence(&about, 60)),
            value: json!({"notes": notes, "history": mem.sender_history}),
            ..Default::default()
        })
    }
}

// ---------------------------------------------------------------- low tools

/// Stable short key of a draft's instructions, so retrying the same step finds the draft it already made.
fn instruction_key(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

struct DraftEmail;

#[async_trait]
impl Tool for DraftEmail {
    fn name(&self) -> &'static str {
        "draft_email"
    }
    fn description(&self) -> &'static str {
        "Draft a reply to an email in this case's conversation. Put every fact the reply must contain (candidate names, scores, next steps) in instructions. Never put a sign-off or your own name in instructions; the mailbox owner's signature is added automatically."
    }
    fn schema(&self) -> Value {
        object(
            json!({
                "message_id": {"type": "string", "description": "The email to reply to"},
                "instructions": {"type": "string", "description": "What the reply must say"},
                "quality": {"type": "string", "enum": ["standard", "premium"]}
            }),
            &["message_id"],
        )
    }
    fn risk(&self) -> Risk {
        Risk::Low
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let msg = message_in_ws(ctx, str_arg(args, "message_id")?).await?;
        if msg.direction != "inbound" {
            return failed("Replies can only answer an email we received.");
        }
        if ctx.case.thread_id.as_deref().is_some_and(|t| t != msg.thread_id) {
            return failed("Replies can only go to this case's conversation.");
        }
        let instructions = opt_str(args, "instructions");
        // Without new instructions, reuse an unsent reply the system or an agent suggested for this
        // email. A reply a person wrote or edited is never reused or claimed by an agent.
        let reused: Option<(String, String)> = match instructions {
            Some(_) => None,
            None if ctx.case.shadow => None,
            None => sqlx::query_as(
                "SELECT id, body_text FROM drafts
                 WHERE message_id = ?1 AND workspace_id = ?2 AND status = 'suggested' AND origin_key IS NOT NULL AND send_at IS NULL
                 ORDER BY created_at DESC, id DESC LIMIT 1",
            )
            .bind(&msg.id)
            .bind(ctx.ws())
            .fetch_optional(&ctx.state.db)
            .await?,
        };
        let (draft_id, body) = match reused.clone() {
            Some(d) => d,
            None => {
                let (agent, _) = crate::agent_registry::runtime::context_for_case(&ctx.state.db, &ctx.state.config, ctx.case).await?;
                let quality = match opt_str(args, "quality") {
                    _ if agent.agent.quality == "best" => Some(DraftQuality::Premium),
                    Some("premium") => Some(DraftQuality::Premium),
                    Some("standard") => Some(DraftQuality::Standard),
                    _ if ctx.case.kind == "requirement" => Some(DraftQuality::Premium),
                    _ => None,
                };
                let style = agent.draft_style(&ctx.state.config);
                let owner_said = ctx.case.state().get("owner_said").and_then(|o| {
                    Some((o.get("user_id")?.as_str()?.to_string(), o.get("text")?.as_str()?.to_string()))
                });
                if ctx.case.shadow {
                    let body = crate::agent_profiles::with_draft_style(style, handlers::preview_draft(ctx.state, &msg.id, quality)).await?;
                    if body.trim().is_empty() {
                        return failed("The writer judged that this email needs no reply.");
                    }
                    let gaps = replies::has_open_gaps(&body);
                    let stand_in = format!("shadow:{}:{}", ctx.case.id, ctx.turn);
                    return Ok(ToolOutput {
                        summary: format!("Observe mode: wrote a reply to {} (kept here, not saved as a draft)", msg.sender_address),
                        value: json!({"draft_id": stand_in, "body": body, "has_open_questions": gaps, "shadow": true}),
                        state: Some(json!({"last_draft_id": stand_in})),
                        ..Default::default()
                    });
                }
                let origin = match instructions {
                    Some(text) => format!("agent:{}:{}:{}", ctx.case.id, msg.id, instruction_key(text)),
                    None => format!("agent:{}:{}", ctx.case.id, ctx.turn),
                };
                crate::agent_profiles::with_draft_style(style, handlers::create_agent_draft(ctx.state, &msg.id, instructions, quality, &origin, ctx.case.agent_id.as_deref(), owner_said.as_ref().map(|(u, t)| (u.as_str(), t.as_str()))))
                    .await?
                    .ok_or_else(|| ToolError::Failed("The writer judged that this email needs no reply.".into()))?
            }
        };
        if let Some(agent_id) = &ctx.case.agent_id {
            sqlx::query("UPDATE drafts SET agent_id = ?1 WHERE id = ?2 AND workspace_id = ?3 AND agent_id IS NULL AND status = 'suggested' AND origin_key IS NOT NULL")
                .bind(agent_id)
                .bind(&draft_id)
                .bind(ctx.ws())
                .execute(&ctx.state.db)
                .await?;
        }
        let gaps = replies::has_open_gaps(&body);
        let verb = if reused.is_some() { "Used the suggested reply to" } else { "Drafted a reply to" };
        Ok(ToolOutput {
            summary: format!("{verb} {}{}", msg.sender_address, if gaps { " (it still has open questions)" } else { "" }),
            state: Some(json!({"last_draft_id": draft_id})),
            value: json!({"draft_id": draft_id, "body": body, "has_open_questions": gaps}),
            ..Default::default()
        })
    }
}

struct CreateTask;

#[async_trait]
impl Tool for CreateTask {
    fn name(&self) -> &'static str {
        "create_task"
    }
    fn description(&self) -> &'static str {
        "Ask a person on the team to do something (shows in Reviews)."
    }
    fn schema(&self) -> Value {
        object(json!({"title": {"type": "string"}, "message_id": {"type": "string", "description": "Defaults to the case's email"}}), &["title"])
    }
    fn risk(&self) -> Risk {
        Risk::Low
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let title = sentence(str_arg(args, "title")?, 200);
        let Some(message_id) = opt_str(args, "message_id").or(ctx.case.message_id.as_deref()) else {
            return failed("A task needs an email to attach to.");
        };
        let msg = message_in_ws(ctx, message_id).await?;
        let review_id = new_id("rev");
        let mut tx = ctx.state.db.begin_with("BEGIN IMMEDIATE").await?;
        let created = sqlx::query(
            "INSERT INTO review_items (id, workspace_id, message_id, workflow_run_id, kind, title, status, created_at)
             SELECT ?1, ?2, ?3, NULL, 'agent_task', ?4, 'open', ?5
             WHERE NOT EXISTS (SELECT 1 FROM review_items WHERE message_id = ?3 AND kind = 'agent_task' AND title = ?4 AND status = 'open')",
        )
        .bind(&review_id)
        .bind(ctx.ws())
        .bind(&msg.id)
        .bind(&title)
        .bind(now_ms())
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if created {
            audit::record(
                &mut tx,
                Entry {
                    workspace_id: ctx.ws(),
                    actor: Actor::System,
                    action: "review.created",
                    entity_type: "review_item",
                    entity_id: &review_id,
                    message_id: Some(&msg.id),
                    metadata: json!({"kind": "agent_task", "case_id": ctx.case.id}),
                },
            )
            .await?;
        }
        tx.commit().await?;
        Ok(ToolOutput {
            summary: format!("Asked the team: {title}"),
            value: json!({"created": created, "title": title}),
            ..Default::default()
        })
    }
}

pub const MAX_SLEEP_HOURS: i64 = 14 * 24;

struct ScheduleCheck;

#[async_trait]
impl Tool for ScheduleCheck {
    fn name(&self) -> &'static str {
        "schedule_check"
    }
    fn description(&self) -> &'static str {
        "Pause the case and look again after some hours (a reply in the conversation wakes it earlier)."
    }
    fn schema(&self) -> Value {
        object(json!({"hours": {"type": "integer", "description": "1 to 336"}, "reason": {"type": "string"}}), &["hours", "reason"])
    }
    fn risk(&self) -> Risk {
        Risk::Low
    }
    async fn run(&self, _ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let hours = args.get("hours").and_then(Value::as_i64).unwrap_or(0);
        if !(1..=MAX_SLEEP_HOURS).contains(&hours) {
            return failed("hours must be between 1 and 336");
        }
        let reason = sentence(str_arg(args, "reason")?, 160);
        let wake = now_ms() + hours * 3_600_000;
        Ok(ToolOutput {
            summary: format!("Will check again in {hours} hour{}: {reason}", plural(hours as usize)),
            value: json!({"wake_at": iso(wake)}),
            sleep_until: Some(wake),
            ..Default::default()
        })
    }
}

struct UpdateCase;

#[async_trait]
impl Tool for UpdateCase {
    fn name(&self) -> &'static str {
        "update_case"
    }
    fn description(&self) -> &'static str {
        "Record progress on the case and, when known, its outcome."
    }
    fn schema(&self) -> Value {
        object(
            json!({"status_note": {"type": "string"}, "outcome": {"type": "string", "enum": super::OUTCOMES}}),
            &["status_note"],
        )
    }
    fn risk(&self) -> Risk {
        Risk::Low
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let note = sentence(str_arg(args, "status_note")?, 240);
        let outcome = opt_str(args, "outcome");
        if outcome.is_some_and(|o| !super::OUTCOMES.contains(&o)) {
            return failed("outcome must be one of submitted, interview, rejected, placed, closed");
        }
        sqlx::query("UPDATE cases SET status_note = ?1, outcome = COALESCE(?2, outcome), updated_at = ?3 WHERE id = ?4 AND workspace_id = ?5")
            .bind(&note)
            .bind(outcome)
            .bind(now_ms())
            .bind(&ctx.case.id)
            .bind(ctx.ws())
            .execute(&ctx.state.db)
            .await?;
        let summary = match outcome {
            Some(o) => format!("Recorded the outcome as {o}: {note}"),
            None => format!("Progress: {note}"),
        };
        Ok(ToolOutput { summary, value: json!({"status_note": note, "outcome": outcome}), ..Default::default() })
    }
}

struct ExtractFields;

#[async_trait]
impl Tool for ExtractFields {
    fn name(&self) -> &'static str {
        "extract_fields"
    }
    fn description(&self) -> &'static str {
        "Read named details (an order number, a budget, a date) out of one email. Details the email does not state come back missing. \
         fields: [{name, type: string|number|date|boolean, description}]."
    }
    fn schema(&self) -> Value {
        object(
            json!({
                "message_id": {"type": "string", "description": "The email to read"},
                "fields": {"type": "array", "items": {"type": "object"}, "description": "1 to 12 details to find"}
            }),
            &["message_id", "fields"],
        )
    }
    fn risk(&self) -> Risk {
        Risk::Read
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let msg = message_in_ws(ctx, str_arg(args, "message_id")?).await?;
        if msg.direction != "inbound" {
            return failed("Details can only be read from an email we received.");
        }
        let fields: Vec<crate::agent_registry::spec::ExtractField> = serde_json::from_value(args.get("fields").cloned().unwrap_or(Value::Null))
            .map_err(|_| ToolError::Failed("The details to find are not in the expected shape.".into()))?;
        if fields.is_empty() || fields.len() > 12 {
            return failed("Ask for 1 to 12 details.");
        }
        if !fields.iter().all(crate::agent_registry::spec::field_ok) {
            return failed("Each detail needs a lower-case name and a description of at most 200 characters.");
        }
        let extracted = ctx
            .state
            .field_extractor
            .extract(&msg.subject, &knowledge::clean_body(&msg.body_text), &fields)
            .await
            .map_err(|_| ToolError::Failed("The details could not be read right now.".into()))?;
        let found = extracted.found;
        let missing: Vec<&str> = fields.iter().map(|f| f.name.as_str()).filter(|n| !found.contains_key(*n)).collect();
        let list = |names: Vec<String>| if names.is_empty() { "nothing".to_string() } else { names.join(", ") };
        Ok(ToolOutput {
            summary: format!(
                "Read the email: found {}; missing {}",
                list(found.iter().map(|(k, v)| format!("{k} = {v}")).collect()),
                list(missing.iter().map(|m| m.to_string()).collect())
            ),
            value: json!({"found": found, "missing": missing}),
            state: Some(json!({"extracted": found, "extract_missing": missing})),
            cost_usd: extracted.cost_usd,
            ..Default::default()
        })
    }
}

struct RecordTimesheet;

#[async_trait]
impl Tool for RecordTimesheet {
    fn name(&self) -> &'static str {
        "record_timesheet"
    }
    fn description(&self) -> &'static str {
        "Read a timesheet from an email (the text, or its attached files) and keep it as a record for the person and period. \
         A new timesheet for the same period replaces the old one. Nothing is sent."
    }
    fn schema(&self) -> Value {
        object(json!({"message_id": {"type": "string", "description": "The timesheet email"}}), &["message_id"])
    }
    fn risk(&self) -> Risk {
        Risk::Low
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let msg = message_in_ws(ctx, str_arg(args, "message_id")?).await?;
        if msg.direction != "inbound" {
            return failed("Timesheets can only be read from an email we received.");
        }
        if ctx.case.shadow {
            return Ok(ToolOutput {
                summary: "Observe mode: would have recorded the timesheet; nothing was done".into(),
                value: json!({"simulated": true}),
                ..Default::default()
            });
        }
        let run = crate::staffing::timesheet_records::record_message(ctx.state, ctx.ws(), &msg.id).await?;
        let clean = !run.records.is_empty() && !run.needs_attention();
        let summary = match run.records.as_slice() {
            [] if run.deferred => "The timesheet could not be read just now; it is tried again later".to_string(),
            [] if run.waiting_for_files => "The attached files are still being read; the timesheet is recorded when they are".to_string(),
            [] => "No timesheet data was found in this email".to_string(),
            records => records
                .iter()
                .map(|r| {
                    let who = r.person_name.clone().unwrap_or_else(|| "someone not yet matched to a person".into());
                    let hours = r.total_hours.map_or("no total".to_string(), |h| format!("{h} h"));
                    format!("Recorded {who}, {} to {}, {hours} ({})", r.period_start, r.period_end, r.status.replace('_', " "))
                })
                .collect::<Vec<_>>()
                .join("; "),
        };
        Ok(ToolOutput {
            summary,
            value: json!({"records": run.records, "waiting_for_files": run.waiting_for_files}),
            state: Some(json!({"timesheet_clean": clean})),
            ..Default::default()
        })
    }
}

struct LabelEmail;

#[async_trait]
impl Tool for LabelEmail {
    fn name(&self) -> &'static str {
        "label_email"
    }
    fn description(&self) -> &'static str {
        "Give an email one of this workspace's labels (a person's own label is never overwritten)."
    }
    fn schema(&self) -> Value {
        object(json!({"message_id": {"type": "string"}, "label": {"type": "string", "description": "A label key"}}), &["message_id", "label"])
    }
    fn risk(&self) -> Risk {
        Risk::Low
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let msg = message_in_ws(ctx, str_arg(args, "message_id")?).await?;
        let label = str_arg(args, "label")?;
        if ctx.case.shadow {
            return Ok(ToolOutput {
                summary: format!("Observe mode: would have labelled it {label}; nothing was done"),
                value: json!({"simulated": true}),
                ..Default::default()
            });
        }
        if !crate::labels::is_active(&ctx.state.db, ctx.ws(), label).await? {
            return failed("That label does not exist in this workspace.");
        }
        let existing: Option<(String, String, bool, String)> =
            sqlx::query_as("SELECT category, priority, requires_reply, source FROM classifications WHERE message_id = ?1")
                .bind(&msg.id)
                .fetch_optional(&ctx.state.db)
                .await?;
        let Some((category, priority, requires_reply, source)) = existing else { return failed("This email has not been labelled yet.") };
        if source == "manual" {
            return Ok(ToolOutput {
                summary: format!("Kept the label a person chose ({category})"),
                value: json!({"label": category, "kept": true}),
                ..Default::default()
            });
        }
        let out = crate::ai::ClassifyOutput {
            category: label.to_string(),
            priority: crate::domain::Priority::parse(&priority).unwrap_or(crate::domain::Priority::Normal),
            requires_reply,
            confidence: None,
            source: crate::domain::ClassificationSource::Llm,
            model_id: Some("agent".into()),
            model_version: None,
        };
        handlers::persist_classification(ctx.state, &msg.id, &out, ctx.actor, None, handlers::WriteMode::Upsert).await?;
        Ok(ToolOutput { summary: format!("Labelled the email {label}"), value: json!({"label": label}), ..Default::default() })
    }
}

struct SaveToGmailDrafts;

#[async_trait]
impl Tool for SaveToGmailDrafts {
    fn name(&self) -> &'static str {
        "save_to_gmail_drafts"
    }
    fn description(&self) -> &'static str {
        "Save a drafted reply to the mailbox's Drafts folder so a person can finish it there. Nothing is sent."
    }
    fn schema(&self) -> Value {
        object(json!({"draft_id": {"type": "string"}}), &["draft_id"])
    }
    fn risk(&self) -> Risk {
        Risk::Low
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let draft_id = str_arg(args, "draft_id")?;
        if ctx.case.shadow {
            return Ok(ToolOutput {
                summary: "Observe mode: would have saved the draft to Gmail Drafts; nothing was done".into(),
                value: json!({"simulated": true}),
                ..Default::default()
            });
        }
        let rc = replies::load(ctx.state, ctx.ws(), draft_id).await?;
        if !crate::ai::policy::can_create_provider_draft(&replies::capabilities(&rc)) {
            return Ok(ToolOutput {
                summary: "This mailbox cannot hold Gmail drafts, so the draft stays in Post Room".into(),
                value: json!({"saved": false}),
                ..Default::default()
            });
        }
        let provider_draft_id = replies::save_draft(ctx.state, ctx.ws(), draft_id, ctx.actor).await?;
        Ok(ToolOutput { summary: "Saved the draft to Gmail Drafts".into(), value: json!({"saved": true, "provider_draft_id": provider_draft_id}), ..Default::default() })
    }
}


// ---------------------------------------------------------------- high tools (run on approval)

#[derive(sqlx::FromRow)]
struct DraftRow {
    id: String,
    body_text: String,
    status: String,
    subject: String,
    sender_address: String,
    thread_id: String,
}

async fn draft_preview(ctx: &ToolCtx<'_>, draft_id: &str) -> Result<Preview, ToolError> {
    let d: DraftRow = sqlx::query_as(
        "SELECT d.id, d.body_text, d.status, m.subject, m.sender_address, m.thread_id
         FROM drafts d JOIN email_messages m ON m.id = d.message_id WHERE d.id = ?1 AND d.workspace_id = ?2",
    )
    .bind(draft_id)
    .bind(ctx.ws())
    .fetch_optional(&ctx.state.db)
    .await?
    .ok_or_else(|| ToolError::Failed("That draft was not found in this workspace.".into()))?;
    Ok(Preview {
        summary: format!("Send reply to {}", d.sender_address),
        to: Some(d.sender_address),
        subject: Some(replies::reply_subject(&d.subject)),
        body: d.body_text,
        draft_id: Some(d.id),
        draft_status: Some(d.status),
        thread_id: Some(d.thread_id),
        ..Default::default()
    })
}

struct SendEmail;

#[async_trait]
impl Tool for SendEmail {
    fn name(&self) -> &'static str {
        "send_email"
    }
    fn description(&self) -> &'static str {
        "Send a drafted reply from the connected mailbox. A person approves it first."
    }
    fn schema(&self) -> Value {
        object(json!({"draft_id": {"type": "string"}}), &["draft_id"])
    }
    fn risk(&self) -> Risk {
        Risk::High
    }
    async fn preview(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<Option<Preview>, ToolError> {
        Ok(Some(draft_preview(ctx, str_arg(args, "draft_id")?).await?))
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let preview = draft_preview(ctx, str_arg(args, "draft_id")?).await?;
        let draft_id = preview.draft_id.clone().unwrap_or_default();
        let scheduled = replies::send_draft_held(ctx.state, ctx.ws(), &draft_id, ctx.actor, ctx.hold_secs).await?;
        Ok(ToolOutput {
            summary: format!("Reply to {} is sending (can be undone for a few seconds)", preview.to.unwrap_or_default()),
            value: json!({"scheduled": true, "draft_id": draft_id, "send_at": scheduled.send_at, "send_at_ms": scheduled.send_at_ms}),
            state: Some(json!({"last_sent_draft_id": draft_id})),
            ..Default::default()
        })
    }
}

struct SubmitCandidate;

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

struct ProposeAutomation;

impl ProposeAutomation {
    /// The suggestion as a plain-English job for the designer. Automatic sending is described as
    /// "reply for review" because a designed agent earns sending later; nothing here can switch it on.
    fn describe(input: &crate::automations::AutomationInput, reason: &str) -> String {
        use crate::automations::{Action, Condition, ContainsOp, LabelOp};
        let when: Vec<String> = input
            .conditions
            .iter()
            .map(|c| match c {
                Condition::Label { op: LabelOp::Is, value } => format!("the label is {value}"),
                Condition::Label { op: LabelOp::IsNot, value } => format!("the label is not {value}"),
                Condition::Priority { value, .. } => format!("the priority is {value:?}").to_lowercase(),
                Condition::NeedsReply { value, .. } => if *value { "it needs a reply".to_string() } else { "it needs no reply".to_string() },
                Condition::From { op: ContainsOp::Contains, value } => format!("the sender contains \"{value}\""),
                Condition::Subject { op: ContainsOp::Contains, value } => format!("the subject contains \"{value}\""),
            })
            .collect();
        let then: Vec<String> = input
            .actions
            .iter()
            .map(|a| match a {
                Action::DraftReply { instructions: Some(i) } => format!("draft a reply ({})", sentence(i, 160)),
                Action::DraftReply { instructions: None } => "draft a reply".to_string(),
                Action::SaveToGmailDrafts => "save the draft to Gmail Drafts".to_string(),
                Action::SendReply { .. } => "send the reply (here: draft it and ask a person before sending)".to_string(),
                Action::CreateReview { title: Some(t) } => format!("ask a person to review: {}", sentence(t, 100)),
                Action::CreateReview { title: None } => "ask a person to review".to_string(),
                Action::RemindIfNoReply { hours } => format!("remind a person if nobody replied after {hours} hours"),
            })
            .collect();
        let mut text = format!("An agent suggested a standing rule called \"{}\". ", sentence(input.name.trim(), 80));
        if !when.is_empty() {
            text.push_str(&format!("When {}: ", when.join(" and ")));
        }
        text.push_str(&format!("{}.", then.join(", then ")));
        if !reason.is_empty() {
            text.push_str(&format!(" Why: {reason}"));
        }
        text
    }

    fn parse(args: &Value) -> Result<crate::automations::AutomationInput, ToolError> {
        let input = json!({
            "name": args.get("name").cloned().unwrap_or(Value::Null),
            "enabled": true,
            "conditions": args.get("conditions").cloned().unwrap_or_else(|| json!([])),
            "actions": args.get("actions").cloned().unwrap_or_else(|| json!([])),
        });
        let input: crate::automations::AutomationInput = serde_json::from_value(input)
            .map_err(|e| ToolError::Failed(format!("The automation is not in the expected shape: {}", sentence(&e.to_string(), 160))))?;
        crate::automations::validate(&input.name, &input.conditions, &input.actions).map_err(ToolError::Failed)?;
        Ok(input)
    }
}

#[async_trait]
impl Tool for ProposeAutomation {
    fn name(&self) -> &'static str {
        "propose_automation"
    }
    fn description(&self) -> &'static str {
        "Suggest a standing rule for emails like this one. If an admin approves, it opens the agent designer; nothing runs until they review, dry-run and save it. \
         conditions: [{field: label, op: is|is_not, value: <label key>} | {field: priority, op: is, value: high|normal|low} | \
         {field: needs_reply, op: is, value: bool} | {field: from|subject, op: contains, value: text}]. \
         actions: [{type: draft_reply, instructions?} | {type: save_to_gmail_drafts} | {type: create_review, title?} | \
         {type: remind_if_no_reply, hours} | {type: send_reply, daily_cap, allowed_domains}]."
    }
    fn schema(&self) -> Value {
        object(
            json!({
                "name": {"type": "string", "description": "Short name, e.g. \"Acknowledge timesheets\""},
                "conditions": {"type": "array", "items": {"type": "object"}},
                "actions": {"type": "array", "items": {"type": "object"}},
                "reason": {"type": "string", "description": "One sentence: why this would help the team"}
            }),
            &["name", "actions"],
        )
    }
    fn risk(&self) -> Risk {
        // Never activates on its own: a person approves the suggestion.
        Risk::High
    }
    async fn preview(&self, _ctx: &ToolCtx<'_>, args: &Value) -> Result<Option<Preview>, ToolError> {
        let input = Self::parse(args)?;
        let reason = opt_str(args, "reason").map(|r| sentence(r, 240)).unwrap_or_default();
        Ok(Some(Preview { summary: format!("Suggests an automation: {}", sentence(input.name.trim(), 80)), body: reason, ..Default::default() }))
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let input = Self::parse(args)?;
        let Actor::User(admin_id) = ctx.actor else {
            return failed("Only an admin who approves the suggestion can open the agent designer.");
        };
        let reason = opt_str(args, "reason").map(|r| sentence(r, 240)).unwrap_or_default();
        let session = crate::designer::session::open_from_proposal(ctx.state, ctx.ws(), admin_id, &input.name, &Self::describe(&input, &reason)).await?;
        audit::record(
            &mut *ctx.state.db.acquire().await?,
            Entry {
                workspace_id: ctx.ws(),
                actor: ctx.actor,
                action: "designer.session_opened",
                entity_type: "designer_session",
                entity_id: &session.id,
                message_id: None,
                metadata: json!({ "by": "agent", "case_id": ctx.case.id, "drafted": session.spec.is_some() }),
            },
        )
        .await?;
        let name = sentence(input.name.trim(), 80);
        Ok(ToolOutput {
            summary: format!("Opened the agent designer for \"{name}\". Nothing runs until an admin reviews, dry-runs and saves it"),
            value: json!({"designer_session_id": session.id, "drafted": session.spec.is_some()}),
            ..Default::default()
        })
    }
}

// ---------------------------------------------------------------- messaging

struct SendMessage;

impl SendMessage {
    async fn target(ctx: &ToolCtx<'_>, args: &Value) -> Result<crate::messaging::recipients::Target, ToolError> {
        crate::messaging::recipients::resolve(&ctx.state.db, ctx.ws(), str_arg(args, "to")?).await.map_err(ToolError::Failed)
    }

    fn channel(args: &Value) -> crate::messaging::dispatch::Channel {
        if opt_str(args, "channel") == Some("in_app") {
            crate::messaging::dispatch::Channel::InApp
        } else {
            crate::messaging::dispatch::Channel::Email
        }
    }
}

#[async_trait]
impl Tool for SendMessage {
    fn name(&self) -> &'static str {
        "send_message"
    }
    fn description(&self) -> &'static str {
        "Send a NEW message (not a reply) to a person, a team or a contact: channel email goes from the owner's mailbox, in_app shows in a member's Post Room. \
         Teams are messaged in the app. Set ask to true to wait for the answer and report it back to the owner. \
         Messages to people inside the workspace may go on their own once this assistant has earned it; anything outside the workspace, \
         about money, banks or security, with attachments, or after text that tried to instruct you always waits for a person."
    }
    fn schema(&self) -> Value {
        object(
            json!({
                "channel": {"type": "string", "enum": ["email", "in_app"]},
                "to": {"type": "string", "description": "An email address, a member's name, or a team name"},
                "subject": {"type": "string"},
                "body": {"type": "string", "description": "The message, in plain sentences"},
                "ask": {"type": "boolean", "description": "Wait for the answer and report it back"}
            }),
            &["channel", "to", "subject", "body"],
        )
    }
    fn risk(&self) -> Risk {
        Risk::High
    }
    async fn preview(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<Option<Preview>, ToolError> {
        let target = Self::target(ctx, args).await?;
        let subject = str_arg(args, "subject")?;
        let via = if matches!(Self::channel(args), crate::messaging::dispatch::Channel::InApp) || matches!(target, crate::messaging::recipients::Target::Team { .. }) {
            "in the app"
        } else {
            "by email"
        };
        Ok(Some(Preview {
            summary: format!("Send “{}” to {} {via}", sentence(subject, 60), target.label()),
            to: Some(target.people().iter().map(|p| p.address.clone().unwrap_or_else(|| p.label.clone())).collect::<Vec<_>>().join(", ")),
            subject: Some(subject.to_string()),
            body: str_arg(args, "body")?.to_string(),
            ..Default::default()
        }))
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let target = Self::target(ctx, args).await?;
        let subject = str_arg(args, "subject")?;
        let body = str_arg(args, "body")?;
        if ctx.case.shadow {
            return Ok(ToolOutput {
                summary: "Observe mode: would have sent a message; nothing was done".into(),
                value: json!({"simulated": true}),
                ..Default::default()
            });
        }
        let approver = match ctx.actor {
            Actor::User(u) => Some(u),
            Actor::System => None,
        };
        let owner = ctx.case.agent_user_id.as_deref();
        let ask = args.get("ask").and_then(Value::as_bool) == Some(true);
        let sent = crate::messaging::dispatch::send(
            ctx.state,
            crate::messaging::dispatch::Request {
                ws: ctx.ws(),
                owner,
                agent_id: ctx.case.agent_id.as_deref(),
                channel: Self::channel(args),
                target: &target,
                subject,
                body,
                approver,
                hold_secs: ctx.hold_secs,
                watch: (ask && owner.is_some())
                    .then_some(crate::messaging::dispatch::WatchSpec { question: body, nudge_hours: crate::messaging::watch::DEFAULT_NUDGE_HOURS }),
                support: None,
                via: "agent",
            },
        )
        .await?;
        Ok(ToolOutput {
            summary: sent.summary.clone(),
            value: json!({"sent": true, "link": sent.link, "send_at_ms": sent.send_at_ms, "watching": sent.watch_id.is_some()}),
            state: Some(json!({"last_message_to": target.label()})),
            ..Default::default()
        })
    }
}

struct NotifyOwner;

#[async_trait]
impl Tool for NotifyOwner {
    fn name(&self) -> &'static str {
        "notify_owner"
    }
    fn description(&self) -> &'static str {
        "Tell the person this assistant works for something, in their Post Room notifications (for example \"Support replied\" or \"3 emails need attention\"). \
         Set email_line to true to also email them a one-line version."
    }
    fn schema(&self) -> Value {
        object(
            json!({
                "subject": {"type": "string", "description": "A short headline"},
                "body": {"type": "string", "description": "One or two plain sentences"},
                "email_line": {"type": "boolean"}
            }),
            &["subject"],
        )
    }
    fn risk(&self) -> Risk {
        Risk::Low
    }
    async fn run(&self, ctx: &ToolCtx<'_>, args: &Value) -> Result<ToolOutput, ToolError> {
        let subject = str_arg(args, "subject")?;
        if ctx.case.shadow {
            return Ok(ToolOutput {
                summary: "Observe mode: would have notified the owner; nothing was done".into(),
                value: json!({"simulated": true}),
                ..Default::default()
            });
        }
        let Some(owner) = ctx.case.agent_user_id.as_deref() else {
            return failed("This case has no owner to notify.");
        };
        let link = ctx.case.message_id.as_deref().map_or(json!({"kind": "meera"}), |m| json!({"kind": "message", "id": m}));
        let id = crate::messaging::inapp::notify_owner(
            ctx.state,
            ctx.ws(),
            owner,
            ctx.case.agent_id.as_deref(),
            subject,
            opt_str(args, "body").unwrap_or_default(),
            link,
            args.get("email_line").and_then(Value::as_bool) == Some(true),
        )
        .await?;
        Ok(ToolOutput { summary: format!("Told the owner: {}", sentence(subject, 80)), value: json!({"notified": true, "notice_id": id}), ..Default::default() })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    const TYPES: &[&str] = &["string", "integer", "number", "boolean", "array", "object"];

    fn assert_valid_object_schema(name: &str, schema: &Value) {
        assert_eq!(schema["type"], "object", "{name}");
        assert_eq!(schema["additionalProperties"], false, "{name}");
        let props = schema["properties"].as_object().unwrap_or_else(|| panic!("{name}: properties"));
        assert!(!props.is_empty(), "{name}");
        for (key, prop) in props {
            let t = prop["type"].as_str().unwrap_or_else(|| panic!("{name}.{key}: type"));
            assert!(TYPES.contains(&t), "{name}.{key}: {t}");
            if t == "array" {
                assert!(prop["items"]["type"].is_string(), "{name}.{key}: items");
            }
            if let Some(e) = prop.get("enum") {
                assert!(e.as_array().is_some_and(|e| !e.is_empty() && e.iter().all(Value::is_string)), "{name}.{key}: enum");
            }
        }
        for req in schema["required"].as_array().unwrap_or_else(|| panic!("{name}: required")) {
            assert!(props.contains_key(req.as_str().unwrap()), "{name}: required {req} not in properties");
        }
    }

    #[test]
    fn every_tool_has_a_valid_schema_and_unique_name() {
        let reg = registry();
        let mut names = HashSet::new();
        for t in reg.all() {
            assert!(t.name().chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{}", t.name());
            assert!(names.insert(t.name()), "duplicate {}", t.name());
            assert!(!t.description().is_empty());
            assert_valid_object_schema(t.name(), &t.schema());
        }
        let expected = [
            "search_mail", "read_email", "read_thread", "find_candidates", "find_requirements", "recall_memory", "draft_email", "create_task",
            "schedule_check", "update_case", "extract_fields", "record_timesheet", "label_email", "save_to_gmail_drafts", "send_email", "submit_candidate",
            "propose_automation", "send_message", "notify_owner",
        ];
        assert_eq!(names, expected.into_iter().collect());
        let specs = reg.specs();
        assert_eq!(specs.len(), expected.len() + 1);
        for spec in &specs {
            assert_eq!(spec["type"], "function");
            let f = &spec["function"];
            assert_valid_object_schema(f["name"].as_str().unwrap(), &f["parameters"]);
        }
        assert_eq!(specs.last().unwrap()["function"]["name"], "finish");
    }

    #[test]
    fn risks_follow_the_contract() {
        let reg = registry();
        let risk = |n: &str| reg.get(n).unwrap().risk();
        for n in ["search_mail", "read_email", "read_thread", "find_candidates", "recall_memory", "extract_fields"] {
            assert_eq!(risk(n), Risk::Read, "{n}");
        }
        for n in ["draft_email", "create_task", "schedule_check", "update_case", "label_email", "save_to_gmail_drafts", "record_timesheet"] {
            assert_eq!(risk(n), Risk::Low, "{n}");
        }
        assert_eq!((risk("send_email"), risk("submit_candidate")), (Risk::High, Risk::High));
        assert_eq!(risk("propose_automation"), Risk::High, "suggestions wait for a person");
    }

    #[test]
    fn validates_arguments_against_schemas() {
        let reg = registry();
        let submit = reg.get("submit_candidate").unwrap().schema();
        assert!(validate_args(&submit, &json!({"requirement_id": "r", "candidate_ids": ["a"], "draft_id": "d"})).is_ok());
        assert!(validate_args(&submit, &json!({"requirement_id": "r", "draft_id": "d"})).is_err());
        assert!(validate_args(&submit, &json!({"requirement_id": "r", "candidate_ids": "a", "draft_id": "d"})).is_err());
        assert!(validate_args(&submit, &json!([1])).is_err());
        let update = reg.get("update_case").unwrap().schema();
        assert!(validate_args(&update, &json!({"status_note": "ok", "outcome": "interview"})).is_ok());
        assert!(validate_args(&update, &json!({"status_note": "ok", "outcome": "hired"})).is_err());
        let check = reg.get("schedule_check").unwrap().schema();
        assert!(validate_args(&check, &json!({"hours": "48", "reason": "x"})).is_err());
        assert!(validate_args(&check, &json!({"hours": 48, "reason": "x", "extra": 1})).is_ok());
    }
}
