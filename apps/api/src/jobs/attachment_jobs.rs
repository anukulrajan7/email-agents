//! `extract_attachments`: reads every attachment of a message into text (spreadsheets, Office, PDF/DOCX, images,
//! audio), stores it with how reading went, then re-indexes the email and refreshes its insight.
//!
//! Readers that need no model always run. Images and audio call a model, so they follow the same rule as the
//! other AI stages: new mail only, within the daily budget, and for older mail when someone opens the email.
//! Provider errors are recorded on the attachment; they never fail the message.

use serde_json::{Value, json};
use sqlx::SqliteConnection;

use super::{Job, handlers::{self, JobError}, kind, matching_jobs::download_gmail_attachment};
use crate::{
    mail::attachments::{self, Kind, audio, vision},
    http::AppState,
    jobs,
    mail::sources::normalize::truncate_chars,
};

const MAX_PER_MESSAGE: i64 = 20;
/// New attachment text must add at least this much before the insight is read again.
pub const REFRESH_MIN_CHARS: usize = 200;

/// Queues reading of this message's attachments. `open` marks a person opening an older email.
pub async fn enqueue_extract(conn: &mut SqliteConnection, workspace_id: &str, message_id: &str, open: bool) -> sqlx::Result<bool> {
    let key = if open { format!("attachments:{message_id}:open") } else { format!("attachments:{message_id}") };
    jobs::enqueue(conn, kind::EXTRACT_ATTACHMENTS, &key, Some(workspace_id), &json!({ "message_id": message_id })).await
}

#[derive(sqlx::FromRow)]
struct SourceRow {
    workspace_id: String,
    source_message_id: String,
    direction: String,
    source_kind: String,
    connection_id: Option<String>,
    provider_config_key: Option<String>,
}

#[derive(sqlx::FromRow)]
struct AttachmentRow {
    id: String,
    filename: String,
    media_type: Option<String>,
    size_bytes: Option<i64>,
    provider_attachment_id: Option<String>,
    text_status: String,
    text_content: Option<String>,
}

enum Outcome {
    Done { kind: Kind, text: String, meta: Value },
    Failed { kind: Kind, error: String },
    Skipped { kind: Kind, error: String },
}

fn skipped(kind: Kind, why: &str) -> Outcome {
    Outcome::Skipped { kind, error: why.into() }
}

fn failed(kind: Kind, why: &str) -> Outcome {
    Outcome::Failed { kind, error: truncate_chars(why, 200) }
}

pub async fn extract_attachments(state: &AppState, job: &Job, payload: &Value) -> Result<(), JobError> {
    let message_id = payload.get("message_id").and_then(Value::as_str).ok_or_else(|| JobError::Permanent("missing message_id".into()))?;
    let Some(msg) = sqlx::query_as::<_, SourceRow>(
        "SELECT m.workspace_id, m.source_message_id, m.direction, s.kind AS source_kind,
                s.external_connection_id AS connection_id, s.provider_config_key
         FROM email_messages m JOIN email_sources s ON s.id = m.source_id WHERE m.id = ?1",
    )
    .bind(message_id)
    .fetch_optional(&state.db)
    .await?
    else {
        return Ok(());
    };
    if msg.direction == "outbound" {
        return Ok(());
    }
    let person = handlers::person_initiated(&job.dedupe_key);
    let ai_allowed = !handlers::ai_deferred(state, job, payload, true).await?;
    let rows: Vec<AttachmentRow> = sqlx::query_as(
        "SELECT id, filename, media_type, size_bytes, provider_attachment_id, text_status, text_content FROM attachments
         WHERE message_id = ?1 AND (extract_status IS NULL
                                    OR (extract_status = 'skipped' AND extract_kind IN ('image', 'audio'))
                                    OR (extract_status = 'failed' AND ?2))
         ORDER BY filename LIMIT ?3",
    )
    .bind(message_id)
    .bind(person)
    .bind(MAX_PER_MESSAGE)
    .fetch_all(&state.db)
    .await?;

    let mut with_text = 0;
    let mut added_chars = 0;
    for a in &rows {
        let outcome = read_one(state, job, &msg, a, ai_allowed).await?;
        if let Outcome::Done { text, .. } = &outcome
            && !text.is_empty()
        {
            with_text += 1;
            added_chars += text.chars().count();
        }
        store(state, &a.id, outcome).await?;
    }
    tracing::info!(%message_id, attachments = rows.len(), with_text, ai_allowed, "attachments read");
    if !rows.is_empty() {
        enqueue_timesheet_record(state, job, &msg.workspace_id, message_id).await?;
    }
    if with_text == 0 {
        return Ok(());
    }

    let open = if person { ":open" } else { "" };
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    jobs::enqueue(
        &mut tx,
        kind::INDEX_MESSAGE,
        &format!("index:{message_id}:media{open}:{}", job.id),
        Some(&msg.workspace_id),
        &json!({ "message_id": message_id }),
    )
    .await?;
    // At most one refresh per email (and one more when a person opens it), and only for real new text.
    if added_chars >= REFRESH_MIN_CHARS {
        jobs::enqueue(
            &mut tx,
            kind::ANALYZE_MESSAGE,
            &format!("analyze:{message_id}:attachments{open}"),
            Some(&msg.workspace_id),
            &json!({ "message_id": message_id, "refresh_attachments": true }),
        )
        .await?;
    }
    tx.commit().await?;
    state.jobs_notify.notify_one();
    Ok(())
}

/// A timesheet email's files were just read (or could not be): read the timesheet again with them.
async fn enqueue_timesheet_record(state: &AppState, job: &Job, workspace_id: &str, message_id: &str) -> Result<(), JobError> {
    let labelled: Option<String> = sqlx::query_scalar("SELECT category FROM classifications WHERE message_id = ?1").bind(message_id).fetch_optional(&state.db).await?;
    if labelled.as_deref() != Some(crate::hr::staffing::timesheet_records::LABEL) || !crate::platform::packs::enabled(&state.db, workspace_id, crate::platform::packs::STAFFING).await? {
        return Ok(());
    }
    let mut conn = state.db.acquire().await?;
    jobs::enqueue(&mut conn, kind::RECORD_TIMESHEET, &format!("timesheet:{message_id}:files:{}", job.id), Some(workspace_id), &json!({ "message_id": message_id })).await?;
    // The desk's case may be waiting for these files: it looks again now instead of at its next check.
    if crate::platform::flags::mode(&state.db, &state.config, workspace_id, crate::platform::flags::Key::Handoff).await?.acts() {
        crate::agents::triggers::wake_for_message(&mut conn, workspace_id, message_id).await?;
    }
    state.jobs_notify.notify_one();
    Ok(())
}

async fn read_one(state: &AppState, job: &Job, msg: &SourceRow, a: &AttachmentRow, ai_allowed: bool) -> Result<Outcome, JobError> {
    let kind = attachments::classify(&a.filename, a.media_type.as_deref());
    if kind == Kind::Unsupported {
        return Ok(skipped(kind, "This file type is not supported"));
    }
    if matches!(kind, Kind::Pdf | Kind::Docx)
        && a.text_status == "extracted"
        && let Some(text) = a.text_content.as_deref().filter(|t| !t.trim().is_empty())
    {
        return Ok(Outcome::Done { kind, text: text.to_string(), meta: json!({}) });
    }
    if kind.needs_ai() {
        if !ai_allowed {
            return Ok(skipped(kind, "Open this email to have AI read this file"));
        }
        let ready = if kind == Kind::Image { state.vision.is_some() } else { state.audio.is_some() };
        if !ready {
            return Ok(skipped(kind, "AI file reading is not set up"));
        }
    }
    if a.size_bytes.is_some_and(|s| s as usize > kind.max_bytes()) {
        return Ok(skipped(kind, "This file is too large to read"));
    }

    let bytes = match (msg.source_kind.as_str(), &a.provider_attachment_id, &msg.connection_id, &msg.provider_config_key) {
        ("nango_gmail", Some(att), Some(conn), Some(pck)) => {
            match download_gmail_attachment(state, &msg.source_message_id, conn, pck, att).await? {
                Some(bytes) => bytes,
                None => return Ok(failed(kind, "The file could not be downloaded")),
            }
        }
        _ => return Ok(skipped(kind, "Files on forwarded mail can't be read yet")),
    };
    if bytes.len() > kind.max_bytes() {
        return Ok(skipped(kind, "This file is too large to read"));
    }

    match kind {
        Kind::Image => read_image(state, job, &bytes).await,
        Kind::Audio => read_audio(state, job, a, &bytes).await,
        _ => match attachments::read_document_blocking(kind, a.filename.clone(), a.media_type.clone(), bytes).await {
            Ok(p) if p.text.trim().is_empty() => Ok(skipped(kind, "No readable text found")),
            Ok(p) => Ok(Outcome::Done { kind, text: p.text, meta: p.meta }),
            Err(e) => Ok(failed(kind, &e)),
        },
    }
}

/// Retry a provider hiccup until the last attempt, then record the failure on the attachment.
fn ai_failure(job: &Job, kind: Kind, e: crate::ai::AiError, friendly: &str) -> Result<Outcome, JobError> {
    match e {
        crate::ai::AiError::Retryable(m) if job.attempts < job.max_attempts => Err(JobError::Retry(m)),
        e => {
            tracing::warn!(error = %e, kind = kind.as_str(), "AI could not read attachment");
            Ok(failed(kind, friendly))
        }
    }
}

async fn read_image(state: &AppState, job: &Job, bytes: &[u8]) -> Result<Outcome, JobError> {
    let Some(reader) = &state.vision else { return Ok(skipped(Kind::Image, "AI file reading is not set up")) };
    let Some(media_type) = vision::sniff_media_type(bytes) else {
        return Ok(failed(Kind::Image, "This is not a PNG, JPEG or WebP image"));
    };
    match reader.read_image(media_type, bytes).await {
        Ok(reading) => {
            let p = vision::to_parsed(&reading);
            Ok(Outcome::Done { kind: Kind::Image, text: attachments::clean_text(&p.text), meta: p.meta })
        }
        Err(e) => ai_failure(job, Kind::Image, e, "AI could not read this image"),
    }
}

async fn read_audio(state: &AppState, job: &Job, a: &AttachmentRow, bytes: &[u8]) -> Result<Outcome, JobError> {
    let Some(reader) = &state.audio else { return Ok(skipped(Kind::Audio, "AI file reading is not set up")) };
    match audio::read_audio(reader.as_ref(), &a.filename, a.media_type.as_deref(), bytes).await {
        Ok(p) if p.text.trim().is_empty() => Ok(skipped(Kind::Audio, "No speech found")),
        Ok(p) => Ok(Outcome::Done { kind: Kind::Audio, text: attachments::clean_text(&p.text), meta: p.meta }),
        Err(e) => ai_failure(job, Kind::Audio, e, "AI could not transcribe this recording"),
    }
}

async fn store(state: &AppState, id: &str, outcome: Outcome) -> Result<(), JobError> {
    let (kind, status, error, text, meta, terminal) = match outcome {
        Outcome::Done { kind, text, meta } => (kind, "done", None, Some(text).filter(|t| !t.is_empty()), Some(meta), None),
        Outcome::Failed { kind, error } => (kind, "failed", Some(error), None, None, Some("failed")),
        Outcome::Skipped { kind, error } => {
            let terminal = (kind == Kind::Unsupported).then_some("unsupported");
            (kind, "skipped", Some(error), None, None, terminal)
        }
    };
    sqlx::query(
        "UPDATE attachments SET extract_kind = ?2, extract_status = ?3, scan_status = ?3, extract_error = ?4, extract_meta_json = ?5,
           text_content = CASE WHEN ?6 IS NOT NULL THEN ?6 ELSE text_content END,
           text_status = CASE WHEN ?6 IS NOT NULL THEN 'extracted'
                              WHEN ?7 IS NOT NULL AND text_status = 'pending' THEN ?7 ELSE text_status END
         WHERE id = ?1",
    )
    .bind(id)
    .bind(kind.as_str())
    .bind(status)
    .bind(error)
    .bind(meta.map(|m| m.to_string()))
    .bind(text)
    .bind(terminal)
    .execute(&state.db)
    .await?;
    Ok(())
}
