use super::*;

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
