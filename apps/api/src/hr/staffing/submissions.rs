//! Submissions: a recruiter's sent mail to a client with a CV attached. Detected from stored outbound mail,
//! linked to a candidate and requirement (names, matching records, embeddings) and followed to an outcome
//! from the client's replies. Counting is exact SQL over this table; only the link step uses similarity,
//! and an ambiguous link waits for a person to confirm it.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::{name_key, policy::address_of};
use crate::{
    platform::db::{new_id, now_ms},
    http::AppState,
    hr::matching::SIMILARITY_CLOSE,
    hr::team::{access, settings},
};

pub const LOOKBACK_MS: i64 = 60 * 86_400_000;
const MAX_PER_SCAN: i64 = 300;
const CV_EXTENSIONS: &[&str] = &["pdf", "doc", "docx", "rtf", "odt"];
const NOT_A_CV: &[&str] = &["invoice", "timesheet", "offer", "payslip", "jd", "job_description", "requirement"];
const FREE_MAIL: &[&str] = &[
    "gmail.com", "googlemail.com", "yahoo.com", "yahoo.in", "yahoo.co.in", "outlook.com", "hotmail.com", "live.com", "icloud.com", "proton.me",
    "protonmail.com", "aol.com", "rediffmail.com", "zoho.com",
];

pub fn is_cv(filename: &str) -> bool {
    let lower = filename.to_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    CV_EXTENSIONS.contains(&ext) && !NOT_A_CV.iter().any(|w| lower.contains(w))
}

fn domain_of(address: &str) -> Option<&str> {
    address.rsplit_once('@').map(|(_, d)| d).filter(|d| d.contains('.'))
}

/// "hr.capgemini.com" and "capgemini.co.uk" both give "capgemini".
pub fn org_label(domain: &str) -> String {
    let parts: Vec<&str> = domain.split('.').collect();
    let n = parts.len();
    let label = if n >= 3 && ["co", "com", "org", "net", "ac", "gov"].contains(&parts[n - 2]) { parts[n - 3] } else if n >= 2 { parts[n - 2] } else { domain };
    label.to_lowercase()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Interview,
    Rejected,
    Selected,
    Joined,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Interview => "interview",
            Outcome::Rejected => "rejected",
            Outcome::Selected => "selected",
            Outcome::Joined => "joined",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [Outcome::Interview, Outcome::Rejected, Outcome::Selected, Outcome::Joined].into_iter().find(|o| o.as_str() == s)
    }
}

/// What a client's reply says about a submission, by plain phrases (no model). Order matters: a rejection
/// that mentions "selected" ("not selected") is still a rejection.
pub fn classify_outcome(text: &str) -> Option<Outcome> {
    let t = text.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| t.contains(w));
    if has(&["has joined", "have joined", "joined us", "joining formalities", "onboarded", "joining confirmed", "date of joining"]) {
        return Some(Outcome::Joined);
    }
    if has(&[
        "reject", "not shortlisted", "not suitable", "not moving forward", "not a fit", "not selected", "regret", "unfortunately", "does not match",
        "profile not", "not proceed", "not interested",
    ]) {
        return Some(Outcome::Rejected);
    }
    if has(&["selected for the next", "selected for interview", "selected for an interview", "shortlisted for interview", "shortlisted for the interview"]) {
        return Some(Outcome::Interview);
    }
    if has(&["offer letter", "offer has been", "offer released", "we have selected", "has been selected", "is selected", "cleared all rounds", "go ahead with"]) {
        return Some(Outcome::Selected);
    }
    if has(&["interview", "technical round", "l1 round", "l2 round", "schedule a call", "available for a call", "slot"]) {
        return Some(Outcome::Interview);
    }
    None
}

// ---------------------------------------------------------------- scan

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct ScanReport {
    pub scanned: usize,
    pub created: usize,
    pub needs_confirm: usize,
}

#[derive(sqlx::FromRow)]
struct Outbound {
    id: String,
    thread_id: String,
    source_id: String,
    sender_address: String,
    recipients_json: String,
    cc_json: String,
    subject: String,
    body_text: String,
    received_at: i64,
}

fn parse_list(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default().iter().map(|a| address_of(a)).filter(|a| !a.is_empty()).collect()
}

/// Looks at stored outbound mail from people in the directory that has not been scanned yet.
pub async fn scan_workspace(state: &AppState, workspace_id: &str, now: i64) -> sqlx::Result<ScanReport> {
    let db = &state.db;
    let cfg = settings::load(db, workspace_id).await?;
    let rows: Vec<Outbound> = sqlx::query_as(
        "SELECT m.id, m.thread_id, m.source_id, m.sender_address, m.recipients_json, m.cc_json, m.subject, m.body_text, m.received_at
         FROM email_messages m
         WHERE m.workspace_id = ?1 AND m.direction = 'outbound' AND m.received_at >= ?2
           AND EXISTS (SELECT 1 FROM attachments a WHERE a.message_id = m.id)
           AND NOT EXISTS (SELECT 1 FROM submission_scans s WHERE s.message_id = m.id)
           AND (lower(m.sender_address) IN (SELECT email FROM person_emails WHERE workspace_id = ?1)
                OR m.source_id IN (SELECT source_id FROM people WHERE workspace_id = ?1 AND active = 1 AND source_id IS NOT NULL))
         ORDER BY m.received_at LIMIT ?3",
    )
    .bind(workspace_id)
    .bind(now - LOOKBACK_MS)
    .bind(MAX_PER_SCAN)
    .fetch_all(db)
    .await?;
    let mut report = ScanReport::default();
    if rows.is_empty() {
        return Ok(report);
    }
    let internal: HashSet<String> = sqlx::query_scalar::<_, String>(
        "SELECT email FROM person_emails WHERE workspace_id = ?1
         UNION SELECT lower(u.email) FROM memberships mb JOIN users u ON u.id = mb.user_id WHERE mb.workspace_id = ?1",
    )
    .bind(workspace_id)
    .fetch_all(db)
    .await?
    .into_iter()
    .collect();
    let internal_domains: HashSet<&str> =
        internal.iter().filter_map(|a| domain_of(a)).filter(|d| !FREE_MAIL.contains(d)).collect();
    for m in rows {
        report.scanned += 1;
        let result = scan_one(state, workspace_id, &m, &cfg.client_domains, &internal, &internal_domains, now).await?;
        report.created += result.0;
        report.needs_confirm += result.1;
        sqlx::query("INSERT OR IGNORE INTO submission_scans (message_id, workspace_id, result, scanned_at) VALUES (?1, ?2, ?3, ?4)")
            .bind(&m.id)
            .bind(workspace_id)
            .bind(if result.0 > 0 { "submission" } else { "none" })
            .bind(now)
            .execute(db)
            .await?;
    }
    Ok(report)
}

async fn scan_one(
    state: &AppState,
    workspace_id: &str,
    m: &Outbound,
    client_domains: &[String],
    internal: &HashSet<String>,
    internal_domains: &HashSet<&str>,
    now: i64,
) -> sqlx::Result<(usize, usize)> {
    let db = &state.db;
    let person: Option<String> = match sqlx::query_scalar(
        "SELECT e.person_id FROM person_emails e JOIN people p ON p.id = e.person_id WHERE e.workspace_id = ?1 AND e.email = lower(?2) AND p.active = 1",
    )
    .bind(workspace_id)
    .bind(&m.sender_address)
    .fetch_optional(db)
    .await?
    {
        Some(p) => Some(p),
        None => sqlx::query_scalar("SELECT id FROM people WHERE workspace_id = ?1 AND source_id = ?2 AND active = 1 ORDER BY created_at LIMIT 1")
            .bind(workspace_id)
            .bind(&m.source_id)
            .fetch_optional(db)
            .await?,
    };
    let Some(person_id) = person else { return Ok((0, 0)) };

    let mut recipients = parse_list(&m.recipients_json);
    recipients.extend(parse_list(&m.cc_json));
    let client_address = recipients.into_iter().find(|a| {
        let Some(d) = domain_of(a) else { return false };
        if internal.contains(a) {
            return false;
        }
        if !client_domains.is_empty() {
            return client_domains.iter().any(|c| d == c || d.ends_with(&format!(".{c}")));
        }
        !FREE_MAIL.contains(&d) && !internal_domains.contains(d)
    });
    let Some(client_address) = client_address else { return Ok((0, 0)) };
    let client_domain = domain_of(&client_address).unwrap_or_default().to_string();

    let files: Vec<String> = sqlx::query_scalar("SELECT filename FROM attachments WHERE message_id = ?1 ORDER BY filename").bind(&m.id).fetch_all(db).await?;
    let mut created = 0;
    let mut confirm = 0;
    for (i, file) in files.iter().filter(|f| is_cv(f)).enumerate() {
        let key = match name_key(file) {
            k if k.is_empty() => format!("{}#{i}", m.id),
            k => k,
        };
        let link = link(state, workspace_id, m, &key, &client_domain).await?;
        let status = link.status();
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO submissions
               (id, workspace_id, person_id, message_id, thread_id, sent_at, client_domain, client, attachment_name, candidate_key,
                candidate_name, candidate_id, requirement_id, link_status, link_options_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?16)",
        )
        .bind(new_id("sub"))
        .bind(workspace_id)
        .bind(&person_id)
        .bind(&m.id)
        .bind(&m.thread_id)
        .bind(m.received_at)
        .bind(&client_domain)
        .bind(&link.client)
        .bind(file)
        .bind(&key)
        .bind(&link.candidate_name)
        .bind(&link.candidate_id)
        .bind(&link.requirement_id)
        .bind(status)
        .bind(link.options().to_string())
        .bind(now)
        .execute(db)
        .await?
        .rows_affected();
        created += inserted as usize;
        confirm += (inserted > 0 && status == "needs_confirm") as usize;
    }
    Ok((created, confirm))
}

// ---------------------------------------------------------------- linking

#[derive(Default)]
struct Link {
    candidate_id: Option<String>,
    candidate_name: Option<String>,
    requirement_id: Option<String>,
    client: Option<String>,
    candidate_options: Vec<Value>,
    requirement_options: Vec<Value>,
}

impl Link {
    fn status(&self) -> &'static str {
        let ambiguous = (self.candidate_id.is_none() && !self.candidate_options.is_empty())
            || (self.requirement_id.is_none() && !self.requirement_options.is_empty());
        match (self.candidate_id.is_some(), self.requirement_id.is_some(), ambiguous) {
            (_, _, true) => "needs_confirm",
            (true, _, false) => "auto",
            _ => "unlinked",
        }
    }

    fn options(&self) -> Value {
        json!({ "candidates": self.candidate_options, "requirements": self.requirement_options })
    }
}

fn tokens(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric()).filter(|t| t.chars().count() >= 2).map(str::to_lowercase).collect()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn from_blob(blob: &[u8]) -> Vec<f32> {
    blob.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect()
}

async fn profile_embeddings(db: &SqlitePool, workspace_id: &str, kind: &str) -> sqlx::Result<HashMap<String, Vec<f32>>> {
    let rows: Vec<(String, Vec<u8>)> = sqlx::query_as("SELECT record_id, embedding FROM match_profiles WHERE workspace_id = ?1 AND record_kind = ?2")
        .bind(workspace_id)
        .bind(kind)
        .fetch_all(db)
        .await?;
    Ok(rows.into_iter().map(|(id, blob)| (id, from_blob(&blob))).collect())
}

#[derive(sqlx::FromRow)]
struct Cand {
    id: String,
    name: String,
    email: Option<String>,
}

#[derive(sqlx::FromRow)]
struct Req {
    id: String,
    client: Option<String>,
    role_title: String,
}

async fn link(state: &AppState, workspace_id: &str, m: &Outbound, key: &str, client_domain: &str) -> sqlx::Result<Link> {
    let db = &state.db;
    let mut out = Link::default();
    let body: String = m.body_text.chars().take(1_500).collect();
    let text_lower = format!("{} {}", m.subject, body).to_lowercase();
    let file_tokens = tokens(key);
    let text_tokens = tokens(&text_lower);

    // The mail's own embedding, only when names alone settle nothing.
    let mut mail_vec: Option<Vec<f32>> = None;
    let embed = |text: String| {
        let embedder = state.embedder.clone();
        async move { embedder.embed(&[text]).await.ok().and_then(|mut v| v.pop()) }
    };

    // ---- candidate
    let cands: Vec<Cand> = sqlx::query_as("SELECT id, name, email FROM candidates WHERE workspace_id = ?1 AND status != 'archived' LIMIT 2000")
        .bind(workspace_id)
        .fetch_all(db)
        .await?;
    let mut scored: Vec<(f64, &Cand)> = vec![];
    for c in &cands {
        let ck = name_key(&c.name);
        let name_tokens: Vec<&str> = ck.split(' ').filter(|t| !t.is_empty()).collect();
        if name_tokens.is_empty() {
            continue;
        }
        let in_file = name_tokens.iter().all(|t| file_tokens.contains(*t));
        let in_text = name_tokens.len() >= 2 && name_tokens.iter().all(|t| text_tokens.contains(*t));
        let by_email = c.email.as_deref().is_some_and(|e| !e.is_empty() && text_lower.contains(&e.to_lowercase()));
        let score = if ck == key { 1.0 } else if by_email || (in_file && name_tokens.len() >= 2) { 0.95 } else if in_text { 0.8 } else { 0.0 };
        if score > 0.0 {
            scored.push((score, c));
        }
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let option = |score: f64, c: &Cand| json!({ "id": c.id, "name": c.name, "score": (score * 100.0).round() / 100.0 });
    match scored.as_slice() {
        [] => {
            // Similarity only suggests; it never links by itself.
            let profiles = profile_embeddings(db, workspace_id, "candidate").await?;
            if !profiles.is_empty() {
                mail_vec = embed(format!("{key} {} {body}", m.subject)).await;
                if let Some(v) = &mail_vec {
                    let mut sims: Vec<(f32, &Cand)> = cands.iter().filter_map(|c| profiles.get(&c.id).map(|p| (cosine(v, p), c))).collect();
                    sims.sort_by(|a, b| b.0.total_cmp(&a.0));
                    out.candidate_options = sims.iter().filter(|(s, _)| *s >= SIMILARITY_CLOSE).take(3).map(|(s, c)| option(*s as f64, c)).collect();
                }
            }
        }
        [(_, c)] => {
            out.candidate_id = Some(c.id.clone());
            out.candidate_name = Some(c.name.clone());
        }
        [(a, ca), (b, _), ..] if a - b >= 0.1 => {
            out.candidate_id = Some(ca.id.clone());
            out.candidate_name = Some(ca.name.clone());
        }
        many => out.candidate_options = many.iter().take(3).map(|(s, c)| option(*s, c)).collect(),
    }
    if out.candidate_name.is_none() && out.candidate_options.is_empty() && !key.contains('#') {
        out.candidate_name = Some(key.split(' ').map(|w| w[..1].to_uppercase() + &w[1..]).collect::<Vec<_>>().join(" "));
    }

    // ---- requirement
    let reqs: Vec<Req> = sqlx::query_as("SELECT id, client, role_title FROM requirements WHERE workspace_id = ?1 AND status = 'open' LIMIT 500")
        .bind(workspace_id)
        .fetch_all(db)
        .await?;
    if reqs.is_empty() {
        return Ok(out);
    }
    let org = org_label(client_domain);
    let matched: HashMap<String, String> = match &out.candidate_id {
        Some(cid) => sqlx::query_as::<_, (String, String)>("SELECT requirement_id, status FROM matches WHERE candidate_id = ?1 AND status != 'dismissed'")
            .bind(cid)
            .fetch_all(db)
            .await?
            .into_iter()
            .collect(),
        None => HashMap::new(),
    };
    let profiles = profile_embeddings(db, workspace_id, "requirement").await?;
    if !profiles.is_empty() && mail_vec.is_none() {
        mail_vec = embed(format!("{key} {} {body}", m.subject)).await;
    }
    let mut ranked: Vec<(f64, &Req)> = vec![];
    for r in &reqs {
        let squashed = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
        let client_hit = r.client.as_deref().map(squashed).is_some_and(|c| org.len() >= 3 && (c.contains(&org) || (c.len() >= 3 && org.contains(&c))));
        let role_tokens: HashSet<String> = tokens(&r.role_title).into_iter().filter(|t| t.chars().count() >= 3).collect();
        let role_hit = if role_tokens.is_empty() { 0.0 } else { role_tokens.iter().filter(|t| text_tokens.contains(*t)).count() as f64 / role_tokens.len() as f64 };
        let mut score = if client_hit { 0.5 } else { 0.0 } + 0.3 * role_hit;
        match matched.get(&r.id).map(String::as_str) {
            Some("shortlisted") => score += 0.4,
            Some(_) => score += 0.3,
            None => {}
        }
        if let (Some(v), Some(p)) = (&mail_vec, profiles.get(&r.id)) {
            let sim = cosine(v, p);
            if sim >= SIMILARITY_CLOSE {
                score += 0.2 * (((sim - 0.5) / 0.3).clamp(0.0, 1.0)) as f64;
            }
        }
        if score >= 0.3 {
            ranked.push((score, r));
        }
    }
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    let pick = |r: &Req| {
        (
            r.id.clone(),
            r.client.clone().filter(|c| !c.is_empty()),
        )
    };
    match ranked.as_slice() {
        [] => {}
        [(s, r)] if *s >= 0.7 => {
            (out.requirement_id, out.client) = (Some(pick(r).0), pick(r).1);
        }
        [(a, r), (b, _), ..] if *a >= 0.7 && a - b >= 0.2 => {
            (out.requirement_id, out.client) = (Some(pick(r).0), pick(r).1);
        }
        many => {
            out.requirement_options = many
                .iter()
                .take(3)
                .map(|(s, r)| json!({ "id": r.id, "role_title": r.role_title, "client": r.client, "score": (s * 100.0).round() / 100.0 }))
                .collect();
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- outcomes

#[derive(sqlx::FromRow)]
struct Open {
    id: String,
    thread_id: String,
    sent_at: i64,
    client_domain: String,
    candidate_key: String,
}

#[derive(sqlx::FromRow)]
struct Reply {
    id: String,
    own_thread: bool,
    subject: String,
    body_text: String,
    received_at: i64,
}

/// Re-reads the client's replies for every submission that has not ended (or been set by hand) and updates
/// its outcome. Replies count when they are in the submission's thread, or are labelled `submission_update`
/// from the same client and name the candidate. Returns submissions whose outcome changed.
pub async fn update_outcomes(db: &SqlitePool, workspace_id: &str, now: i64) -> sqlx::Result<usize> {
    let open: Vec<Open> = sqlx::query_as(
        "SELECT id, thread_id, sent_at, client_domain, candidate_key FROM submissions
         WHERE workspace_id = ?1 AND status = 'active' AND outcome != 'joined' AND COALESCE(outcome_message_id, '') != 'manual'
           AND sent_at >= ?2",
    )
    .bind(workspace_id)
    .bind(now - 2 * LOOKBACK_MS)
    .fetch_all(db)
    .await?;
    let mut changed = 0;
    for s in open {
        let replies: Vec<Reply> = sqlx::query_as(
            "SELECT m.id, m.thread_id = ?3 AS own_thread, m.subject, m.body_text, m.received_at FROM email_messages m
             LEFT JOIN classifications c ON c.message_id = m.id
             WHERE m.workspace_id = ?1 AND m.direction = 'inbound' AND m.received_at > ?2
               AND lower(m.sender_address) NOT IN (SELECT email FROM person_emails WHERE workspace_id = ?1)
               AND (m.thread_id = ?3
                    OR (c.category = 'submission_update'
                        AND (lower(m.sender_address) LIKE '%@' || ?4 OR lower(m.sender_address) LIKE '%.' || ?4)))
             ORDER BY m.received_at, m.id",
        )
        .bind(workspace_id)
        .bind(s.sent_at)
        .bind(&s.thread_id)
        .bind(&s.client_domain)
        .fetch_all(db)
        .await?;
        let key_tokens: BTreeSet<&str> = s.candidate_key.split(' ').filter(|t| t.len() >= 2 && !t.contains('#')).collect();
        let (mut outcome, mut interviewed, mut by, mut at) = (None::<Outcome>, false, None::<String>, None::<i64>);
        for r in replies {
            let head: String = r.body_text.chars().take(800).collect();
            let text = format!("{} {head}", r.subject);
            // A reply in another thread must name the candidate.
            if !r.own_thread {
                let lower = text.to_lowercase();
                if key_tokens.is_empty() || !key_tokens.iter().all(|t| lower.contains(t)) {
                    continue;
                }
            }
            if let Some(o) = classify_outcome(&text) {
                interviewed |= matches!(o, Outcome::Interview | Outcome::Selected | Outcome::Joined);
                outcome = Some(o);
                by = Some(r.id.clone());
                at = Some(r.received_at);
            }
        }
        let (new_outcome, new_interviewed) = (outcome.map_or("pending", Outcome::as_str), interviewed);
        let updated = sqlx::query(
            "UPDATE submissions SET outcome = ?1, reached_interview = ?2, outcome_message_id = ?3, outcome_at = ?4, updated_at = ?5
             WHERE id = ?6 AND (outcome != ?1 OR reached_interview != ?2 OR COALESCE(outcome_message_id, '') != COALESCE(?3, ''))",
        )
        .bind(new_outcome)
        .bind(new_interviewed)
        .bind(&by)
        .bind(at)
        .bind(now_ms())
        .bind(&s.id)
        .execute(db)
        .await?
        .rows_affected();
        changed += updated as usize;
    }
    Ok(changed)
}

/// A person confirms or corrects a link, dismisses a false positive, or sets the outcome by hand.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    #[serde(default, deserialize_with = "nullable")]
    pub candidate_id: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub requirement_id: Option<Option<String>>,
    pub dismissed: Option<bool>,
    pub outcome: Option<String>,
}

fn nullable<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Ok(Some(<Option<T> as serde::Deserialize>::deserialize(d)?))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct SubmissionRow {
    pub id: String,
    pub person_id: String,
    pub person_name: String,
    pub message_id: String,
    pub sent_at: i64,
    pub client_domain: String,
    pub client: Option<String>,
    pub attachment_name: String,
    pub candidate_name: Option<String>,
    pub candidate_id: Option<String>,
    pub requirement_id: Option<String>,
    pub requirement_title: Option<String>,
    pub link_status: String,
    pub link_options_json: String,
    pub status: String,
    pub outcome: String,
    pub reached_interview: bool,
}

pub const SELECT: &str = "SELECT s.id, s.person_id, p.name AS person_name, s.message_id, s.sent_at, s.client_domain, s.client, s.attachment_name,
        COALESCE(c.name, s.candidate_name) AS candidate_name, s.candidate_id, s.requirement_id, r.role_title AS requirement_title,
        s.link_status, s.link_options_json, s.status, s.outcome, s.reached_interview
    FROM submissions s JOIN people p ON p.id = s.person_id
    LEFT JOIN candidates c ON c.id = s.candidate_id LEFT JOIN requirements r ON r.id = s.requirement_id";

pub fn row_json(r: SubmissionRow) -> Value {
    json!({
        "id": r.id, "person_id": r.person_id, "person_name": r.person_name, "message_id": r.message_id,
        "sent_at": crate::platform::db::iso(r.sent_at), "client_domain": r.client_domain, "client": r.client,
        "attachment_name": r.attachment_name, "candidate_name": r.candidate_name, "candidate_id": r.candidate_id,
        "requirement_id": r.requirement_id, "requirement_title": r.requirement_title, "link_status": r.link_status,
        "options": serde_json::from_str::<Value>(&r.link_options_json).unwrap_or(Value::Null),
        "status": r.status, "outcome": r.outcome, "reached_interview": r.reached_interview,
    })
}

/// Who may change a submission: the super admin, the person, or anyone above them in the manager chain.
pub fn may_decide(a: &access::Access, person_id: &str) -> bool {
    a.can_see(person_id)
}
