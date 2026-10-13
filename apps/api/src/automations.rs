//! Automations: trigger → conditions → actions. This module owns the types, validation,
//! condition evaluation, presets and default seeding. Execution lives in `jobs::automation_jobs`.

use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::db::{iso, new_id, now_ms};

pub const TRIGGER_EMAIL_RECEIVED: &str = "email_received";
pub const MAX_CONDITIONS: usize = 10;
pub const MAX_ACTIONS: usize = 10;
pub const MAX_AUTOMATIONS: i64 = 50;
pub const MAX_REMIND_HOURS: u32 = 24 * 30;
pub const MAX_DAILY_CAP: u32 = 1_000;
pub const MAX_ALLOWED_DOMAINS: usize = 20;
/// Presets created for every workspace the first time it is seen.
pub const DEFAULT_PRESETS: &[&str] = &["reply_to_questions", "chase_important"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelOp {
    Is,
    IsNot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IsOp {
    Is,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainsOp {
    Contains,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorityValue {
    High,
    Normal,
    Low,
}

impl PriorityValue {
    fn as_str(self) -> &'static str {
        match self {
            PriorityValue::High => "high",
            PriorityValue::Normal => "normal",
            PriorityValue::Low => "low",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "field", rename_all = "snake_case", deny_unknown_fields)]
pub enum Condition {
    Label { op: LabelOp, value: String },
    Priority { op: IsOp, value: PriorityValue },
    NeedsReply { op: IsOp, value: bool },
    From { op: ContainsOp, value: String },
    Subject { op: ContainsOp, value: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    DraftReply {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instructions: Option<String>,
    },
    SaveToGmailDrafts,
    SendReply {
        daily_cap: u32,
        allowed_domains: Vec<String>,
    },
    CreateReview {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    RemindIfNoReply {
        hours: u32,
    },
}

impl Action {
    pub fn type_name(&self) -> &'static str {
        match self {
            Action::DraftReply { .. } => "draft_reply",
            Action::SaveToGmailDrafts => "save_to_gmail_drafts",
            Action::SendReply { .. } => "send_reply",
            Action::CreateReview { .. } => "create_review",
            Action::RemindIfNoReply { .. } => "remind_if_no_reply",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationInput {
    pub name: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub conditions: Vec<Condition>,
    pub actions: Vec<Action>,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationPatch {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub conditions: Option<Vec<Condition>>,
    pub actions: Option<Vec<Action>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Automation {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub trigger: String,
    pub conditions: Vec<Condition>,
    pub actions: Vec<Action>,
    pub preset_key: Option<String>,
    pub run_count: i64,
    pub last_run_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PresetInfo {
    pub key: String,
    pub name: String,
    pub description: String,
    pub automation: PresetAutomation,
}

#[derive(Debug, Clone, Serialize)]
pub struct PresetAutomation {
    pub name: String,
    pub enabled: bool,
    pub conditions: Vec<Condition>,
    pub actions: Vec<Action>,
}

// ---------------------------------------------------------------- validation

fn bad_text(label: &str, value: &str, max: usize) -> Option<String> {
    let n = value.trim().chars().count();
    (n == 0 || n > max).then(|| format!("{label} must be 1-{max} characters"))
}

fn valid_domain(d: &str) -> bool {
    !d.is_empty() && d.len() <= 100 && d.contains('.') && d.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

const FREE_MAIL_DOMAINS: &[&str] = &[
    "gmail.com", "googlemail.com", "outlook.com", "hotmail.com", "live.com", "msn.com", "icloud.com", "me.com", "mac.com",
    "aol.com", "proton.me", "protonmail.com", "protonmail.ch", "pm.me", "mail.com", "gmx.com", "gmx.net", "web.de",
    "zoho.com", "zohomail.com", "tutanota.com", "tuta.io", "hey.com", "fastmail.com", "yandex.com", "yandex.ru",
    "ymail.com", "rocketmail.com", "rediffmail.com", "qq.com", "163.com", "126.com", "hushmail.com", "inbox.com",
    "mail.ru",
];
/// Provider families with many country domains (yahoo.co.in, hotmail.fr, …).
const FREE_MAIL_PREFIXES: &[&str] = &["yahoo.", "hotmail.", "outlook.", "live.", "gmx.", "yandex.", "aol."];

/// Domains where anyone can register an address: never a basis for trusting a sender.
pub fn is_free_mail_domain(domain: &str) -> bool {
    let d = domain.trim().trim_end_matches('.').to_lowercase();
    FREE_MAIL_DOMAINS.contains(&d.as_str()) || FREE_MAIL_PREFIXES.iter().any(|p| d.starts_with(p))
}

/// Someone on the team has already replied to this contact by hand: sent a draft from Post
/// Room themselves, or written to them from the mailbox (synced sent mail that no draft sent).
pub async fn has_human_reply(db: &SqlitePool, workspace_id: &str, contact: &str) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS(
           SELECT 1 FROM audit_events a JOIN email_messages m ON m.id = a.message_id
           WHERE a.workspace_id = ?1 AND a.action = 'draft.sent' AND a.actor_type = 'user' AND lower(m.sender_address) = ?2
         ) OR EXISTS(
           SELECT 1 FROM email_messages o
           WHERE o.workspace_id = ?1 AND o.direction = 'outbound'
             AND EXISTS (SELECT 1 FROM json_each(o.recipients_json) r WHERE lower(r.value) = ?2)
             AND NOT EXISTS (SELECT 1 FROM drafts d WHERE d.workspace_id = ?1 AND d.provider_message_id = o.source_message_id)
         )",
    )
    .bind(workspace_id)
    .bind(contact.trim().to_lowercase())
    .fetch_one(db)
    .await
}

/// Returns the first problem as a human-readable message.
pub fn validate(name: &str, conditions: &[Condition], actions: &[Action]) -> Result<(), String> {
    if let Some(e) = bad_text("name", name, 100) {
        return Err(e);
    }
    if conditions.len() > MAX_CONDITIONS {
        return Err(format!("at most {MAX_CONDITIONS} conditions"));
    }
    if actions.is_empty() {
        return Err("add at least one action".into());
    }
    if actions.len() > MAX_ACTIONS {
        return Err(format!("at most {MAX_ACTIONS} actions"));
    }
    for c in conditions {
        match c {
            Condition::Label { value, .. } => {
                if let Some(e) = bad_text("label condition value", value, 60) {
                    return Err(e);
                }
            }
            Condition::From { value, .. } => {
                if let Some(e) = bad_text("from condition value", value, 200) {
                    return Err(e);
                }
            }
            Condition::Subject { value, .. } => {
                if let Some(e) = bad_text("subject condition value", value, 200) {
                    return Err(e);
                }
            }
            Condition::Priority { .. } | Condition::NeedsReply { .. } => {}
        }
    }
    let mut drafted = false;
    for a in actions {
        match a {
            Action::DraftReply { instructions } => {
                if let Some(i) = instructions
                    && i.chars().count() > 1_000
                {
                    return Err("draft instructions must be at most 1000 characters".into());
                }
                drafted = true;
            }
            Action::SaveToGmailDrafts if !drafted => return Err("save_to_gmail_drafts needs a draft_reply action before it".into()),
            Action::SendReply { .. } if !drafted => return Err("send_reply needs a draft_reply action before it".into()),
            Action::SendReply { daily_cap, allowed_domains } => {
                if *daily_cap == 0 || *daily_cap > MAX_DAILY_CAP {
                    return Err(format!("daily_cap must be between 1 and {MAX_DAILY_CAP}"));
                }
                if allowed_domains.is_empty() || allowed_domains.len() > MAX_ALLOWED_DOMAINS {
                    return Err(format!("allowed_domains needs 1-{MAX_ALLOWED_DOMAINS} sender domains, e.g. acme.com"));
                }
                if let Some(d) = allowed_domains.iter().find(|d| !valid_domain(&d.trim().to_lowercase())) {
                    return Err(format!("\"{d}\" is not a valid domain (use e.g. acme.com)"));
                }
                if let Some(d) = allowed_domains.iter().find(|d| is_free_mail_domain(&d.trim().to_lowercase())) {
                    return Err(format!(
                        "\"{d}\" is a free email provider: anyone can get an address there, so it can't be trusted for automatic replies"
                    ));
                }
            }
            Action::CreateReview { title } => {
                if let Some(t) = title
                    && let Some(e) = bad_text("review title", t, 200)
                {
                    return Err(e);
                }
            }
            Action::RemindIfNoReply { hours } if *hours == 0 || *hours > MAX_REMIND_HOURS => {
                return Err(format!("hours must be between 1 and {MAX_REMIND_HOURS}"));
            }
            Action::SaveToGmailDrafts | Action::RemindIfNoReply { .. } => {}
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- evaluation

/// What conditions look at. Missing classification fields never match positive tests.
#[derive(Debug, Clone, FromRow)]
pub struct MessageContext {
    pub id: String,
    pub subject: String,
    pub sender_address: String,
    pub category: Option<String>,
    pub priority: Option<String>,
    pub needs_reply: Option<bool>,
}

/// Shared SELECT: insight wins over the classifier's `requires_reply`.
pub const CONTEXT_SELECT: &str = "SELECT m.id, m.subject, m.sender_address, c.category, c.priority,
        COALESCE(i.needs_reply, c.requires_reply) AS needs_reply
 FROM email_messages m
 LEFT JOIN classifications c ON c.message_id = m.id
 LEFT JOIN message_insights i ON i.message_id = m.id";

impl Condition {
    pub fn matches(&self, ctx: &MessageContext) -> bool {
        match self {
            Condition::Label { op, value } => {
                let is = ctx.category.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(value.trim()));
                match op {
                    LabelOp::Is => is,
                    LabelOp::IsNot => !is,
                }
            }
            Condition::Priority { value, .. } => ctx.priority.as_deref() == Some(value.as_str()),
            Condition::NeedsReply { value, .. } => ctx.needs_reply == Some(*value),
            Condition::From { value, .. } => ctx.sender_address.to_lowercase().contains(&value.trim().to_lowercase()),
            Condition::Subject { value, .. } => ctx.subject.to_lowercase().contains(&value.trim().to_lowercase()),
        }
    }
}

pub fn matches_all(conditions: &[Condition], ctx: &MessageContext) -> bool {
    conditions.iter().all(|c| c.matches(ctx))
}

// ---------------------------------------------------------------- presets

fn preset_info(key: &str) -> Option<PresetInfo> {
    let draft_and_save = |instructions: Option<&str>| {
        vec![Action::DraftReply { instructions: instructions.map(str::to_string) }, Action::SaveToGmailDrafts]
    };
    let (name, description, conditions, actions) = match key {
        "reply_to_questions" => (
            "Reply to questions",
            "When an email needs an answer, write a reply and save it to your Gmail Drafts.",
            vec![Condition::NeedsReply { op: IsOp::Is, value: true }],
            draft_and_save(None),
        ),
        "acknowledge_timesheets" => (
            "Acknowledge timesheets",
            "When a timesheet arrives, draft a short acknowledgement and save it to your Gmail Drafts.",
            vec![Condition::Label { op: LabelOp::Is, value: "timesheet".into() }],
            draft_and_save(Some("Briefly confirm the timesheet was received and will be reviewed.")),
        ),
        "chase_important" => (
            "Chase important emails",
            "If a high-priority email that needs a reply is still unanswered after 4 hours, add it to Reviews.",
            vec![
                Condition::Priority { op: IsOp::Is, value: PriorityValue::High },
                Condition::NeedsReply { op: IsOp::Is, value: true },
            ],
            vec![Action::RemindIfNoReply { hours: 4 }],
        ),
        "auto_ack_referrals" => (
            "Acknowledge referrals",
            "When a candidate referral arrives, draft a thank-you and save it to your Gmail Drafts.",
            vec![Condition::Label { op: LabelOp::Is, value: "candidate_referral".into() }],
            draft_and_save(Some("Thank the sender for the referral and say the recruiting team will review it.")),
        ),
        _ => return None,
    };
    Some(PresetInfo {
        key: key.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        automation: PresetAutomation { name: name.to_string(), enabled: true, conditions, actions },
    })
}

pub const PRESET_KEYS: &[&str] = &["reply_to_questions", "acknowledge_timesheets", "chase_important", "auto_ack_referrals"];

pub fn presets() -> Vec<PresetInfo> {
    PRESET_KEYS.iter().filter_map(|k| preset_info(k)).collect()
}

pub fn preset(key: &str) -> Option<PresetInfo> {
    preset_info(key)
}

impl From<PresetAutomation> for AutomationInput {
    fn from(p: PresetAutomation) -> Self {
        Self { name: p.name, enabled: p.enabled, conditions: p.conditions, actions: p.actions }
    }
}

// ---------------------------------------------------------------- storage

#[derive(FromRow)]
pub struct AutomationRow {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub trigger: String,
    pub conditions_json: String,
    pub actions_json: String,
    pub preset_key: Option<String>,
    pub run_count: i64,
    pub last_run_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<AutomationRow> for Automation {
    fn from(r: AutomationRow) -> Self {
        Self {
            conditions: serde_json::from_str(&r.conditions_json).unwrap_or_default(),
            actions: serde_json::from_str(&r.actions_json).unwrap_or_default(),
            id: r.id,
            name: r.name,
            enabled: r.enabled,
            trigger: r.trigger,
            preset_key: r.preset_key,
            run_count: r.run_count,
            last_run_at: r.last_run_at.map(iso),
            created_at: iso(r.created_at),
            updated_at: iso(r.updated_at),
        }
    }
}

pub const AUTOMATION_COLUMNS: &str =
    "id, name, enabled, trigger, conditions_json, actions_json, preset_key, run_count, last_run_at, created_at, updated_at";

pub async fn list(db: &SqlitePool, workspace_id: &str) -> sqlx::Result<Vec<Automation>> {
    let rows: Vec<AutomationRow> = sqlx::query_as(&format!(
        "SELECT {AUTOMATION_COLUMNS} FROM automations WHERE workspace_id = ?1 ORDER BY created_at, id"
    ))
    .bind(workspace_id)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn get(db: &SqlitePool, workspace_id: &str, id: &str) -> sqlx::Result<Option<Automation>> {
    let row: Option<AutomationRow> =
        sqlx::query_as(&format!("SELECT {AUTOMATION_COLUMNS} FROM automations WHERE id = ?1 AND workspace_id = ?2"))
            .bind(id)
            .bind(workspace_id)
            .fetch_optional(db)
            .await?;
    Ok(row.map(Into::into))
}

/// Inserts an automation and returns its id. Caller validates first.
pub async fn insert(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    input: &AutomationInput,
    preset_key: Option<&str>,
) -> sqlx::Result<String> {
    let id = new_id("aut");
    let now = now_ms();
    sqlx::query(
        "INSERT INTO automations (id, workspace_id, name, enabled, trigger, conditions_json, actions_json, preset_key, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
    )
    .bind(&id)
    .bind(workspace_id)
    .bind(input.name.trim())
    .bind(input.enabled)
    .bind(TRIGGER_EMAIL_RECEIVED)
    .bind(serde_json::to_string(&input.conditions).unwrap_or_else(|_| "[]".into()))
    .bind(serde_json::to_string(&input.actions).unwrap_or_else(|_| "[]".into()))
    .bind(preset_key)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(id)
}

/// Creates the default automations once per workspace. Returns true if it seeded.
pub async fn seed_defaults(conn: &mut SqliteConnection, workspace_id: &str) -> sqlx::Result<bool> {
    let claimed = sqlx::query("INSERT OR IGNORE INTO automation_seeds (workspace_id, seeded_at) VALUES (?1, ?2)")
        .bind(workspace_id)
        .bind(now_ms())
        .execute(&mut *conn)
        .await?
        .rows_affected()
        == 1;
    if !claimed {
        return Ok(false);
    }
    for key in DEFAULT_PRESETS {
        if let Some(p) = preset_info(key) {
            insert(&mut *conn, workspace_id, &p.automation.into(), Some(key)).await?;
        }
    }
    Ok(true)
}

/// Existing workspaces get their defaults on first look.
pub async fn seed_if_needed(db: &SqlitePool, workspace_id: &str) -> sqlx::Result<()> {
    let seeded: Option<i64> = sqlx::query_scalar("SELECT 1 FROM automation_seeds WHERE workspace_id = ?1")
        .bind(workspace_id)
        .fetch_optional(db)
        .await?;
    if seeded.is_some() {
        return Ok(());
    }
    let mut tx = db.begin().await?;
    seed_defaults(&mut tx, workspace_id).await?;
    tx.commit().await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> MessageContext {
        MessageContext {
            id: "m".into(),
            subject: "Timesheet for September".into(),
            sender_address: "Priya@Acme.com".into(),
            category: Some("timesheet".into()),
            priority: Some("high".into()),
            needs_reply: Some(true),
        }
    }

    #[test]
    fn evaluates_conditions_anded() {
        let c = ctx();
        assert!(Condition::Label { op: LabelOp::Is, value: "timesheet".into() }.matches(&c));
        assert!(Condition::Label { op: LabelOp::IsNot, value: "leave".into() }.matches(&c));
        assert!(Condition::From { op: ContainsOp::Contains, value: "acme.com".into() }.matches(&c));
        assert!(Condition::Subject { op: ContainsOp::Contains, value: "SEPTEMBER".into() }.matches(&c));
        let all = [
            Condition::Priority { op: IsOp::Is, value: PriorityValue::High },
            Condition::NeedsReply { op: IsOp::Is, value: false },
        ];
        assert!(!matches_all(&all, &c));
        assert!(matches_all(&all[..1], &c));
        let bare = MessageContext { category: None, priority: None, needs_reply: None, ..c };
        assert!(!Condition::NeedsReply { op: IsOp::Is, value: true }.matches(&bare));
        assert!(!Condition::Label { op: LabelOp::Is, value: "timesheet".into() }.matches(&bare));
    }

    #[test]
    fn validates_action_order_and_limits() {
        let draft = Action::DraftReply { instructions: None };
        let send = Action::SendReply { daily_cap: 5, allowed_domains: vec!["acme.com".into()] };
        assert!(validate("x", &[], &[draft.clone(), send.clone()]).is_ok());
        assert!(validate("x", &[], std::slice::from_ref(&send)).is_err());
        assert!(validate("x", &[], &[Action::SaveToGmailDrafts, draft.clone()]).is_err());
        assert!(validate("x", &[], &[draft.clone(), Action::SendReply { daily_cap: 0, allowed_domains: vec!["acme.com".into()] }]).is_err());
        assert!(validate("x", &[], &[draft.clone(), Action::SendReply { daily_cap: 5, allowed_domains: vec![] }]).is_err());
        assert!(validate("x", &[], &[Action::RemindIfNoReply { hours: 0 }]).is_err());
        assert!(validate("", &[], std::slice::from_ref(&draft)).is_err());
        assert!(validate("x", &[], &[]).is_err());
        let many = vec![Condition::NeedsReply { op: IsOp::Is, value: true }; 11];
        assert!(validate("x", &many, &[draft]).is_err());
    }

    #[test]
    fn wire_format_matches_contract() {
        let a: Action = serde_json::from_str(r#"{"type":"send_reply","daily_cap":3,"allowed_domains":["acme.com"]}"#).unwrap();
        assert_eq!(a.type_name(), "send_reply");
        assert!(serde_json::from_str::<Action>(r#"{"type":"nuke"}"#).is_err());
        assert!(serde_json::from_str::<Condition>(r#"{"field":"label","op":"contains","value":"x"}"#).is_err());
        assert!(serde_json::from_str::<Condition>(r#"{"field":"needs_reply","op":"is","value":true,"extra":1}"#).is_err());
        let c: Condition = serde_json::from_str(r#"{"field":"priority","op":"is","value":"high"}"#).unwrap();
        assert_eq!(serde_json::to_value(&c).unwrap(), serde_json::json!({"field":"priority","op":"is","value":"high"}));
        assert!(presets().iter().all(|p| validate(&p.automation.name, &p.automation.conditions, &p.automation.actions).is_ok()));
        assert!(presets().iter().all(|p| !p.automation.actions.iter().any(|a| a.type_name() == "send_reply")));
    }
}
