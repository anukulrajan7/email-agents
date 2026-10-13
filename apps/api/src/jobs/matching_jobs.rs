//! Staffing pipeline stages: labelled email → (attachment text) → record → profile embedding → recall
//! (vector top-K + skill overlap) → explainable score → matches.

use std::collections::{HashMap, HashSet};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::Method;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use super::{Job, handlers::JobError, kind};
use crate::{
    ai::{
        embeddings::{EMBEDDING_DIMS, to_blob},
        prepare_text,
    },
    audit::{self, Actor, Entry},
    db::{new_id, now_ms},
    http::AppState,
    jobs, knowledge,
    matching::{self, CandidateFields, MIN_MATCH_SCORE, RequirementFields, TextResult},
    sources::normalize::truncate_chars,
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
                        Some(bytes) => match matching::attachment_text_isolated(a.filename.clone(), a.media_type.clone(), bytes).await {
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
    if !attachments.is_empty() {
        let mut conn = state.db.acquire().await?;
        super::knowledge_jobs::enqueue_reindex_message(&mut conn, &msg.workspace_id, message_id, "attachments").await?;
    }
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

/// Nearest counterparts (by profile embedding) recalled for each new/updated record.
pub const RECALL_K: i64 = 20;
/// Counterparts sharing a skill (exact or related) that are always scored.
const OVERLAP_LIMIT: i64 = 200;
/// Counterparts without a profile (e.g. created before semantic matching) embedded per job.
const PROFILE_BACKFILL: i64 = 16;

fn counterpart(record_kind: &str) -> &'static str {
    if record_kind == "requirement" { "candidate" } else { "requirement" }
}

/// `(table, open status)` of a record kind. Our own constants, safe to inline.
fn table(record_kind: &str) -> (&'static str, &'static str) {
    if record_kind == "requirement" { ("requirements", "open") } else { ("candidates", "active") }
}

/// Subject, the new part of the body, and extracted attachment text (CV / JD) of a record's source email.
async fn source_text(db: &SqlitePool, message_id: &str) -> sqlx::Result<String> {
    let msg: Option<(String, String)> = sqlx::query_as("SELECT subject, body_text FROM email_messages WHERE id = ?1")
        .bind(message_id)
        .fetch_optional(db)
        .await?;
    let attachments: Vec<String> = sqlx::query_scalar(
        "SELECT text_content FROM attachments WHERE message_id = ?1 AND text_status = 'extracted' AND text_content IS NOT NULL
         ORDER BY filename LIMIT 3",
    )
    .bind(message_id)
    .fetch_all(db)
    .await?;
    let mut text = msg.map(|(subject, body)| format!("{}\n{}", subject.trim(), knowledge::clean_body(&body))).unwrap_or_default();
    for a in attachments {
        text.push('\n');
        text.push_str(&knowledge::tidy(&a));
    }
    Ok(truncate_chars(&text, matching::PROFILE_SOURCE_CHARS))
}

/// The text embedded for a record (redacted like every model input), or `None` if it no longer exists.
async fn profile_text(db: &SqlitePool, record_kind: &str, id: &str) -> sqlx::Result<Option<String>> {
    let (tbl, _) = table(record_kind);
    let message_id: Option<String> =
        sqlx::query_scalar(&format!("SELECT message_id FROM {tbl} WHERE id = ?1")).bind(id).fetch_optional(db).await?;
    let Some(message_id) = message_id else {
        return Ok(None);
    };
    let source = source_text(db, &message_id).await?;
    let text = if record_kind == "requirement" {
        let r: RequirementRow = sqlx::query_as(&format!("SELECT {REQ_COLUMNS} FROM requirements WHERE id = ?1")).bind(id).fetch_one(db).await?;
        matching::requirement_profile(&r.fields(), &source)
    } else {
        let c: CandidateRow = sqlx::query_as(&format!("SELECT {CAND_COLUMNS} FROM candidates WHERE id = ?1")).bind(id).fetch_one(db).await?;
        matching::candidate_profile(&c.fields(), &source)
    };
    Ok(Some(prepare_text(&text)))
}

fn profile_hash(model: &str, text: &str) -> String {
    format!("{:x}", Sha256::new().chain_update(model.as_bytes()).chain_update([0x1f]).chain_update(text.as_bytes()).finalize())
}

/// Embeds the profiles of these records that are new or changed (one provider call, outside any transaction),
/// then stores them for pairwise similarity and KNN recall. Returns how many were (re-)embedded.
async fn ensure_profiles(state: &AppState, ws: &str, records: &[(&'static str, String)]) -> Result<usize, JobError> {
    let model = &state.config.openrouter_embedding_model_id;
    let mut stale = vec![];
    for (record_kind, id) in records {
        let Some(text) = profile_text(&state.db, record_kind, id).await? else { continue };
        let hash = profile_hash(model, &text);
        let current: Option<String> = sqlx::query_scalar("SELECT content_hash FROM match_profiles WHERE record_kind = ?1 AND record_id = ?2")
            .bind(record_kind)
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
        if current.as_deref() != Some(hash.as_str()) {
            stale.push((*record_kind, id, text, hash));
        }
    }
    if stale.is_empty() {
        return Ok(0);
    }
    let texts: Vec<String> = stale.iter().map(|s| s.2.clone()).collect();
    let vectors = state.embedder.embed(&texts).await?;
    if vectors.len() != stale.len() || vectors.iter().any(|v| v.len() != EMBEDDING_DIMS) {
        return Err(JobError::Permanent("embedder returned the wrong number or size of vectors".into()));
    }
    let now = now_ms();
    let mut tx = state.db.begin().await?;
    for ((record_kind, id, _, hash), vector) in stale.iter().zip(&vectors) {
        let blob = to_blob(vector);
        let rowid: i64 = sqlx::query_scalar(
            "INSERT INTO match_profiles (workspace_id, record_kind, record_id, content_hash, embedding, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(record_kind, record_id) DO UPDATE SET content_hash = excluded.content_hash,
               embedding = excluded.embedding, updated_at = excluded.updated_at
             RETURNING id",
        )
        .bind(ws)
        .bind(record_kind)
        .bind(id)
        .bind(hash)
        .bind(&blob)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM match_profile_vec WHERE rowid = ?1").bind(rowid).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO match_profile_vec (rowid, workspace_id, record_kind, embedding) VALUES (?1, ?2, ?3, ?4)")
            .bind(rowid)
            .bind(ws)
            .bind(record_kind)
            .bind(&blob)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(stale.len())
}

/// Stored profile embeddings by record id.
async fn embeddings(db: &SqlitePool, record_kind: &str, ids: &[String]) -> sqlx::Result<HashMap<String, Vec<f32>>> {
    let rows: Vec<(String, Vec<u8>)> = sqlx::query_as(
        "SELECT record_id, embedding FROM match_profiles WHERE record_kind = ?1 AND record_id IN (SELECT value FROM json_each(?2))",
    )
    .bind(record_kind)
    .bind(json!(ids).to_string())
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|(_, blob)| blob.len() == EMBEDDING_DIMS * 4)
        .map(|(id, blob)| (id, blob.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()))
        .collect())
}

/// Cosine similarity of two L2-normalised vectors.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>().clamp(-1.0, 1.0)
}

/// Open counterparts to score: the top `RECALL_K` by profile similarity within the workspace, plus every one
/// sharing a skill (exact, alias or related). Nearest first, deduplicated.
async fn recall(db: &SqlitePool, ws: &str, record_kind: &str, vector: Option<&[f32]>, skills: &[String]) -> sqlx::Result<Vec<String>> {
    let other = counterpart(record_kind);
    let (tbl, open) = table(other);
    let mut ids: Vec<String> = vec![];
    if let Some(v) = vector {
        ids = sqlx::query_scalar(&format!(
            "WITH knn AS (
               SELECT rowid, distance FROM match_profile_vec
               WHERE embedding MATCH ?1 AND k = ?2 AND workspace_id = ?3 AND record_kind = '{other}'
             )
             SELECT p.record_id FROM knn JOIN match_profiles p ON p.id = knn.rowid
             WHERE EXISTS (SELECT 1 FROM {tbl} x WHERE x.id = p.record_id AND x.workspace_id = ?3 AND x.status = '{open}')
             ORDER BY knn.distance LIMIT ?4"
        ))
        .bind(to_blob(v))
        .bind(RECALL_K * 2)
        .bind(ws)
        .bind(RECALL_K)
        .fetch_all(db)
        .await?;
    }
    let terms = crate::skills::overlap_terms(skills);
    if !terms.is_empty() {
        let has_skill = |column: &str| {
            format!("EXISTS (SELECT 1 FROM json_each(x.{column}) j WHERE lower(trim(j.value)) IN (SELECT value FROM json_each(?2)))")
        };
        let condition = if other == "requirement" {
            format!("({} OR {})", has_skill("must_have_skills_json"), has_skill("nice_to_have_skills_json"))
        } else {
            has_skill("skills_json")
        };
        let overlap: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT x.id FROM {tbl} x WHERE x.workspace_id = ?1 AND x.status = '{open}' AND {condition}
             ORDER BY x.created_at DESC LIMIT ?3"
        ))
        .bind(ws)
        .bind(json!(terms).to_string())
        .bind(OVERLAP_LIMIT)
        .fetch_all(db)
        .await?;
        let seen: HashSet<String> = ids.iter().cloned().collect();
        ids.extend(overlap.into_iter().filter(|id| !seen.contains(id)));
    }
    Ok(ids)
}

pub struct ScoredPair {
    pub requirement_id: String,
    pub candidate_id: String,
    pub score: u8,
    pub reasons: Vec<matching::Reason>,
}

/// Hook for the planned LLM rerank (M4 follow-up): a model would reorder the shortlist and add a written
/// reason per pair. Today the explainable scores pass through unchanged.
fn llm_rerank(scored: Vec<ScoredPair>) -> Vec<ScoredPair> {
    scored
}

/// Refreshes this record's profile embedding, recalls counterparts (vector top-K + skill overlap) and scores
/// each pair with the explainable score. Embedding failures are retried; on the last attempt matching
/// degrades to skill-overlap recall without the semantic component.
pub async fn match_record(state: &AppState, job: &Job, payload: &Value) -> Result<(), JobError> {
    let record_kind: &'static str = match payload.get("kind").and_then(Value::as_str).unwrap_or_default() {
        "requirement" => "requirement",
        "candidate" => "candidate",
        _ => return Err(JobError::Permanent("unknown record kind".into())),
    };
    let id = payload.get("id").and_then(Value::as_str).ok_or_else(|| JobError::Permanent("missing id".into()))?;
    let (own_table, _) = table(record_kind);
    let ws: String = sqlx::query_scalar(&format!("SELECT workspace_id FROM {own_table} WHERE id = ?1"))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| JobError::Permanent("record not found".into()))?;
    let other = counterpart(record_kind);
    let (other_table, other_open) = table(other);

    let missing: Vec<String> = sqlx::query_scalar(&format!(
        "SELECT x.id FROM {other_table} x WHERE x.workspace_id = ?1 AND x.status = '{other_open}'
           AND NOT EXISTS (SELECT 1 FROM match_profiles p WHERE p.record_kind = '{other}' AND p.record_id = x.id)
         ORDER BY x.created_at DESC LIMIT ?2"
    ))
    .bind(&ws)
    .bind(PROFILE_BACKFILL)
    .fetch_all(&state.db)
    .await?;
    let mut wanted = vec![(record_kind, id.to_string())];
    wanted.extend(missing.into_iter().map(|m| (other, m)));
    if let Err(e) = ensure_profiles(state, &ws, &wanted).await {
        match e {
            JobError::Retry(m) if job.attempts < job.max_attempts => return Err(JobError::Retry(m)),
            e => tracing::warn!(error = %e, "profile embedding failed; matching without semantic fit"),
        }
    }
    let own_vector = embeddings(&state.db, record_kind, &[id.to_string()]).await?.remove(id);

    let now = now_ms();
    let mut scored = vec![];
    if record_kind == "requirement" {
        let req: RequirementRow = sqlx::query_as(&format!("SELECT {REQ_COLUMNS} FROM requirements WHERE id = ?1")).bind(id).fetch_one(&state.db).await?;
        let fields = req.fields();
        let skills = [fields.must_have_skills.clone(), fields.nice_to_have_skills.clone()].concat();
        let ids = recall(&state.db, &ws, record_kind, own_vector.as_deref(), &skills).await?;
        let cands: Vec<CandidateRow> = sqlx::query_as(&format!(
            "SELECT {CAND_COLUMNS} FROM candidates WHERE workspace_id = ?1 AND id IN (SELECT value FROM json_each(?2))"
        ))
        .bind(&ws)
        .bind(json!(ids).to_string())
        .fetch_all(&state.db)
        .await?;
        let vectors = embeddings(&state.db, other, &ids).await?;
        for c in cands {
            let similarity = own_vector.as_deref().zip(vectors.get(&c.id)).map(|(a, b)| cosine(a, b));
            let (score, reasons) = matching::score(&fields, &c.fields(), similarity);
            scored.push(ScoredPair { requirement_id: req.id.clone(), candidate_id: c.id, score, reasons });
        }
    } else {
        let cand: CandidateRow = sqlx::query_as(&format!("SELECT {CAND_COLUMNS} FROM candidates WHERE id = ?1")).bind(id).fetch_one(&state.db).await?;
        let fields = cand.fields();
        let ids = recall(&state.db, &ws, record_kind, own_vector.as_deref(), &fields.skills).await?;
        let reqs: Vec<RequirementRow> = sqlx::query_as(&format!(
            "SELECT {REQ_COLUMNS} FROM requirements WHERE workspace_id = ?1 AND id IN (SELECT value FROM json_each(?2))"
        ))
        .bind(&ws)
        .bind(json!(ids).to_string())
        .fetch_all(&state.db)
        .await?;
        let vectors = embeddings(&state.db, other, &ids).await?;
        for r in reqs {
            let similarity = own_vector.as_deref().zip(vectors.get(&r.id)).map(|(a, b)| cosine(b, a));
            let (score, reasons) = matching::score(&r.fields(), &fields, similarity);
            scored.push(ScoredPair { requirement_id: r.id, candidate_id: cand.id.clone(), score, reasons });
        }
    }
    let recalled = scored.len();
    let scored = llm_rerank(scored);

    let mut tx = state.db.begin().await?;
    let mut kept = 0;
    for pair in scored.into_iter().filter(|p| p.score >= MIN_MATCH_SCORE) {
        // Re-scoring never overrides a recruiter's decision.
        sqlx::query(
            "INSERT INTO matches (id, workspace_id, requirement_id, candidate_id, score, reasons_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(requirement_id, candidate_id) DO UPDATE SET score = excluded.score, reasons_json = excluded.reasons_json,
               updated_at = excluded.updated_at WHERE matches.status = 'suggested'",
        )
        .bind(new_id("mat"))
        .bind(&ws)
        .bind(&pair.requirement_id)
        .bind(&pair.candidate_id)
        .bind(pair.score as i64)
        .bind(serde_json::to_string(&pair.reasons).unwrap_or_default())
        .bind(now)
        .execute(&mut *tx)
        .await?;
        kept += 1;
    }
    tx.commit().await?;
    tracing::info!(record_kind, recalled, kept, semantic = own_vector.is_some(), "matching finished");
    if record_kind == "requirement" {
        crate::agents::triggers::after_requirement_matched(state, &ws, id).await?;
    }
    Ok(())
}
