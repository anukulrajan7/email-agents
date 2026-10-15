use super::*;

fn some_value<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

pub(super) fn parse_list(json: &str, allowed: &[&str]) -> Vec<String> {
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
