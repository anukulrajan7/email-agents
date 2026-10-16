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
    platform::audit::{self, Actor, Entry},
    platform::db::{iso, new_id, now_ms},
    platform::error::AppError,
    http::AppState,
    jobs::handlers::{self, JobError},
    learning::knowledge, learning::memory, replies,
};

mod search_mail;
mod read_email;
mod read_thread;
mod find_candidates;
mod find_requirements;
mod recall_memory;
mod draft_email;
mod create_task;
mod schedule_check;
mod update_case;
mod extract_fields;
mod record_timesheet;
mod label_email;
mod save_to_gmail_drafts;
mod send_email;
mod submit_candidate;
mod propose_automation;
mod send_message;
mod notify_owner;

use search_mail::SearchMail;
use read_email::ReadEmail;
use read_thread::ReadThread;
use find_candidates::FindCandidates;
use find_requirements::FindRequirements;
use recall_memory::RecallMemory;
use draft_email::DraftEmail;
use create_task::CreateTask;
use schedule_check::ScheduleCheck;
pub use schedule_check::MAX_SLEEP_HOURS;
use update_case::UpdateCase;
use extract_fields::ExtractFields;
use record_timesheet::RecordTimesheet;
use label_email::LabelEmail;
use save_to_gmail_drafts::SaveToGmailDrafts;
use send_email::SendEmail;
use send_email::draft_preview;
use submit_candidate::SubmitCandidate;
use propose_automation::ProposeAutomation;
use send_message::SendMessage;
use notify_owner::NotifyOwner;

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
    /// The safety check could not run (provider down); the runner waits and tries the same step again.
    #[error("{0}")]
    Deferred(String),
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
            AppError::CheckDeferred(m) => ToolError::Deferred(m),
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

/// Other names planners use for tools we have.
const ALIASES: &[(&str, &str)] = &[
    ("send_mail", "send_email"),
    ("draft_mail", "draft_email"),
    ("draft_reply", "draft_email"),
    ("read_mail", "read_email"),
    ("search_email", "search_mail"),
    ("search_emails", "search_mail"),
    ("find_candidate", "find_candidates"),
    ("find_requirement", "find_requirements"),
];

/// A tool name the way we store it: trimmed, lower case, words joined by `_`, aliases resolved.
/// Gaps are recorded under this name so one wish is one row.
pub fn canonical_name(raw: &str) -> String {
    let flat = raw.trim().to_lowercase().split(|c: char| c == '-' || c.is_whitespace() || c == '_').filter(|w| !w.is_empty()).collect::<Vec<_>>().join("_");
    ALIASES.iter().find(|(alias, _)| *alias == flat).map_or(flat, |(_, real)| (*real).to_string())
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

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

// ---------------------------------------------------------------- low tools

// ---------------------------------------------------------------- high tools (run on approval)

// ---------------------------------------------------------------- messaging

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
    fn tool_names_are_normalised_and_aliases_resolve() {
        assert_eq!(canonical_name("  Launch-Rocket "), "launch_rocket");
        assert_eq!(canonical_name("Launch   rocket"), "launch_rocket");
        assert_eq!(canonical_name("send_mail"), "send_email");
        assert_eq!(canonical_name("Search Emails"), "search_mail");
        assert_eq!(canonical_name("read_email"), "read_email");
        for (_, real) in ALIASES {
            assert!(registry().get(real).is_some(), "{real}");
        }
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
