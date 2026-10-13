//! Staffing pipeline stages: labelled email → (attachment text) → record → matches.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::Method;
use serde_json::{Value, json};

use super::{handlers::JobError, kind};
use crate::{
    audit::{self, Actor, Entry},
    db::{new_id, now_ms},
    http::AppState,
    jobs,
    matching::{self, CandidateFields, MIN_MATCH_SCORE, RequirementFields, TextResult},
};

pub const REQUIREMENT_LABEL: &str = "client_requirement";
pub const CANDIDATE_LABEL: &str = "candidate_referral";

pub fn is_extractable(label: &str) -> bool {
    label == REQUIREMENT_LABEL || label == CANDIDATE_LABEL
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    workspace_id: String,
    source_message_id: String,
    subject: String,
    sender_address: String,
    body_text: String,
    source_kind: String,
    connection_id: Option<String>,
    provider_config_key: Option<String>,
}

#[derive(sqlx::FromRow)]
struct AttachmentRow {
    id: String,
    filename: String,
    media_type: Option<String>,
    provider_attachment_id: Option<String>,
    text_status: String,
    text_content: Option<String>,
}

/// Downloads (Gmail) and reads PDF/DOCX/TXT attachments once; failures are recorded,
/// never fatal. Attachment bytes are not stored.
async fn read_attachments(state: &AppState, message_id: &str, msg: &MessageRow) -> Result<Vec<(String, String)>, JobError> {
    let rows: Vec<AttachmentRow> = sqlx::query_as(
        "SELECT id, filename, media_type, provider_attachment_id, text_status, text_content
         FROM attachments WHERE message_id = ?1 ORDER BY filename LIMIT 5",
    )
    .bind(message_id)
    .fetch_all(&state.db)
    .await?;
    let mut texts = Vec::new();
    for a in rows {
        if a.text_status == "extracted" {
            texts.extend(a.text_content.map(|t| (a.filename.clone(), t)));
            continue;
        }
        if a.text_status != "pending" {
            continue;
        }
        let (status, text) = match (msg.source_kind.as_str(), &a.provider_attachment_id, &msg.connection_id, &msg.provider_config_key) {
            ("nango_gmail", Some(att), Some(conn), Some(pck)) => {
                let path = format!("messages/{}/attachments/{att}", msg.source_message_id);
                match state.nango.gmail_call(Method::GET, conn, pck, &path, None).await {
                    Ok(v) => match v.get("data").and_then(Value::as_str).and_then(|d| URL_SAFE_NO_PAD.decode(d.trim_end_matches('=')).ok()) {
                        Some(bytes) => match matching::attachment_text(&a.filename, a.media_type.as_deref(), &bytes) {
                            TextResult::Text(t) => ("extracted", Some(t)),
                            TextResult::Unsupported => ("unsupported", None),
                            TextResult::Failed(e) => {
                                tracing::warn!(attachment = %a.id, error = %e, "could not read attachment");
                                ("failed", None)
                            }
                        },
                        None => ("failed", None),
                    },
                    Err(crate::sources::nango::NangoError::Unavailable(m)) => return Err(JobError::Retry(m)),
                    Err(e) => {
                        tracing::warn!(attachment = %a.id, error = %e, "attachment download failed");
                        ("failed", None)
                    }
                }
            }
            _ => ("unsupported", None),
        };
        sqlx::query("UPDATE attachments SET text_status = ?1, text_content = ?2 WHERE id = ?3")
            .bind(status)
            .bind(&text)
            .bind(&a.id)
            .execute(&state.db)
            .await?;
        texts.extend(text.map(|t| (a.filename, t)));
    }
    Ok(texts)
}

pub async fn extract_record(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let message_id = payload.get("message_id").and_then(Value::as_str).ok_or_else(|| JobError::Permanent("missing message_id".into()))?;
    let label = payload.get("label").and_then(Value::as_str).unwrap_or_default();
    let msg: MessageRow = sqlx::query_as(
        "SELECT m.workspace_id, m.source_message_id, m.subject, m.sender_address, m.body_text,
                s.kind AS source_kind, s.external_connection_id AS connection_id, s.provider_config_key
         FROM email_messages m JOIN email_sources s ON s.id = m.source_id WHERE m.id = ?1",
    )
    .bind(message_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| JobError::Permanent("message not found".into()))?;

    let attachments = read_attachments(state, message_id, &msg).await?;
    let mut text = msg.body_text.clone();
    for (name, t) in &attachments {
        text.push_str(&format!("\n\n--- Attachment: {name} ---\n{t}"));
    }
    let text = crate::sources::normalize::truncate_chars(&text, 20_000);
    let domain = msg.sender_address.split_once('@').map(|(_, d)| d.to_string());
    let source = if state.config.openrouter_api_key.is_empty() || state.config.openrouter_classifier_model_id.is_empty() {
        "rules"
    } else {
        "llm"
    };
    let now = now_ms();

    let (record_kind, record_id) = match label {
        REQUIREMENT_LABEL => {
            let r = state.extractor.requirement(&msg.subject, &text, domain.as_deref()).await?;
            let id: String = sqlx::query_scalar(
                "INSERT INTO requirements (id, workspace_id, message_id, client, role_title, must_have_skills_json, nice_to_have_skills_json,
                                           min_experience, max_experience, location, positions, source, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(message_id) DO UPDATE SET client = excluded.client, role_title = excluded.role_title,
                   must_have_skills_json = excluded.must_have_skills_json, nice_to_have_skills_json = excluded.nice_to_have_skills_json,
                   min_experience = excluded.min_experience, max_experience = excluded.max_experience,
                   location = excluded.location, positions = excluded.positions
                 RETURNING id",
            )
            .bind(new_id("req"))
            .bind(&msg.workspace_id)
            .bind(message_id)
            .bind(&r.client)
            .bind(crate::sources::normalize::truncate_chars(&r.role_title, 200))
            .bind(json!(r.must_have_skills).to_string())
            .bind(json!(r.nice_to_have_skills).to_string())
            .bind(r.min_experience)
            .bind(r.max_experience)
            .bind(&r.location)
            .bind(r.positions)
            .bind(source)
            .bind(now)
            .fetch_one(&state.db)
            .await?;
            ("requirement", id)
        }
        CANDIDATE_LABEL => {
            let c = state.extractor.candidate(&msg.subject, &text, &msg.sender_address).await?;
            let id: String = sqlx::query_scalar(
                "INSERT INTO candidates (id, workspace_id, message_id, name, email, skills_json, total_experience, location,
                                         notice_days, referred_by, summary, source, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                 ON CONFLICT(message_id) DO UPDATE SET name = excluded.name, email = excluded.email, skills_json = excluded.skills_json,
                   total_experience = excluded.total_experience, location = excluded.location, notice_days = excluded.notice_days,
                   referred_by = excluded.referred_by, summary = excluded.summary
                 RETURNING id",
            )
            .bind(new_id("cand"))
            .bind(&msg.workspace_id)
            .bind(message_id)
            .bind(crate::sources::normalize::truncate_chars(&c.name, 120))
            .bind(&c.email)
            .bind(json!(c.skills).to_string())
            .bind(c.total_experience)
            .bind(&c.location)
            .bind(c.notice_days)
            .bind(&c.referred_by)
            .bind(c.summary.as_deref().map(|s| crate::sources::normalize::truncate_chars(s, 600)))
            .bind(source)
            .bind(now)
            .fetch_one(&state.db)
            .await?;
            ("candidate", id)
        }
        _ => return Ok(()),
    };

    let mut tx = state.db.begin().await?;
    jobs::enqueue(
        &mut tx,
        kind::MATCH_RECORD,
        &format!("match:{record_kind}:{record_id}:{now}"),
        Some(&msg.workspace_id),
        &json!({ "kind": record_kind, "id": record_id }),
    )
    .await?;
    audit::record(
        &mut tx,
        Entry {
            workspace_id: &msg.workspace_id,
            actor: Actor::System,
            action: "record.extracted",
            entity_type: record_kind,
            entity_id: &record_id,
            message_id: Some(message_id),
            metadata: json!({ "source": source, "attachments_read": attachments.len() }),
        },
    )
    .await?;
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(())
}

#[derive(sqlx::FromRow, Clone)]
pub struct RequirementRow {
    pub id: String,
    pub client: Option<String>,
    pub role_title: String,
    pub must_have_skills_json: String,
    pub nice_to_have_skills_json: String,
    pub min_experience: Option<f64>,
    pub max_experience: Option<f64>,
    pub location: Option<String>,
    pub positions: Option<i64>,
}

impl RequirementRow {
    pub fn fields(&self) -> RequirementFields {
        RequirementFields {
            client: self.client.clone(),
            role_title: self.role_title.clone(),
            must_have_skills: serde_json::from_str(&self.must_have_skills_json).unwrap_or_default(),
            nice_to_have_skills: serde_json::from_str(&self.nice_to_have_skills_json).unwrap_or_default(),
            min_experience: self.min_experience,
            max_experience: self.max_experience,
            location: self.location.clone(),
            positions: self.positions,
        }
    }
}

#[derive(sqlx::FromRow, Clone)]
pub struct CandidateRow {
    pub id: String,
    pub name: String,
    pub email: Option<String>,
    pub skills_json: String,
    pub total_experience: Option<f64>,
    pub location: Option<String>,
    pub notice_days: Option<i64>,
    pub referred_by: Option<String>,
    pub summary: Option<String>,
}

impl CandidateRow {
    pub fn fields(&self) -> CandidateFields {
        CandidateFields {
            name: self.name.clone(),
            email: self.email.clone(),
            skills: serde_json::from_str(&self.skills_json).unwrap_or_default(),
            total_experience: self.total_experience,
            location: self.location.clone(),
            notice_days: self.notice_days,
            referred_by: self.referred_by.clone(),
            summary: self.summary.clone(),
        }
    }
}

pub const REQ_COLUMNS: &str =
    "id, client, role_title, must_have_skills_json, nice_to_have_skills_json, min_experience, max_experience, location, positions";
pub const CAND_COLUMNS: &str = "id, name, email, skills_json, total_experience, location, notice_days, referred_by, summary";

/// Scores one new/updated record against every open counterpart in the workspace.
pub async fn match_record(state: &AppState, payload: &Value) -> Result<(), JobError> {
    let record_kind = payload.get("kind").and_then(Value::as_str).unwrap_or_default();
    let id = payload.get("id").and_then(Value::as_str).ok_or_else(|| JobError::Permanent("missing id".into()))?;
    let ws: String = match record_kind {
        "requirement" => sqlx::query_scalar("SELECT workspace_id FROM requirements WHERE id = ?1"),
        "candidate" => sqlx::query_scalar("SELECT workspace_id FROM candidates WHERE id = ?1"),
        _ => return Err(JobError::Permanent("unknown record kind".into())),
    }
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| JobError::Permanent("record not found".into()))?;

    let pairs: Vec<(RequirementRow, CandidateRow)> = if record_kind == "requirement" {
        let req: RequirementRow = sqlx::query_as(&format!("SELECT {REQ_COLUMNS} FROM requirements WHERE id = ?1")).bind(id).fetch_one(&state.db).await?;
        let cands: Vec<CandidateRow> = sqlx::query_as(&format!(
            "SELECT {CAND_COLUMNS} FROM candidates WHERE workspace_id = ?1 AND status = 'active' ORDER BY created_at DESC LIMIT 500"
        ))
        .bind(&ws)
        .fetch_all(&state.db)
        .await?;
        cands.into_iter().map(|c| (req.clone(), c)).collect()
    } else {
        let cand: CandidateRow = sqlx::query_as(&format!("SELECT {CAND_COLUMNS} FROM candidates WHERE id = ?1")).bind(id).fetch_one(&state.db).await?;
        let reqs: Vec<RequirementRow> = sqlx::query_as(&format!(
            "SELECT {REQ_COLUMNS} FROM requirements WHERE workspace_id = ?1 AND status = 'open' ORDER BY created_at DESC LIMIT 500"
        ))
        .bind(&ws)
        .fetch_all(&state.db)
        .await?;
        reqs.into_iter().map(|r| (r, cand.clone())).collect()
    };

    let now = now_ms();
    let mut tx = state.db.begin().await?;
    let mut kept = 0;
    for (req, cand) in pairs {
        let (score, reasons) = matching::score(&req.fields(), &cand.fields());
        if score < MIN_MATCH_SCORE {
            continue;
        }
        // Re-scoring never overrides a recruiter's decision.
        sqlx::query(
            "INSERT INTO matches (id, workspace_id, requirement_id, candidate_id, score, reasons_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(requirement_id, candidate_id) DO UPDATE SET score = excluded.score, reasons_json = excluded.reasons_json,
               updated_at = excluded.updated_at WHERE matches.status = 'suggested'",
        )
        .bind(new_id("mat"))
        .bind(&ws)
        .bind(&req.id)
        .bind(&cand.id)
        .bind(score as i64)
        .bind(serde_json::to_string(&reasons).unwrap_or_default())
        .bind(now)
        .execute(&mut *tx)
        .await?;
        kept += 1;
    }
    tx.commit().await?;
    tracing::info!(record_kind, kept, "matching finished");
    Ok(())
}
