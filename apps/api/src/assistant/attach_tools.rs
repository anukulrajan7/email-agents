//! `list_attachments` and `read_attachment`: the planner reads what the attachment pipeline already
//! extracted (`attachments.text_content`), within a character budget, fenced as untrusted file content.
//! Files that have not been read yet (older mail, AI files waiting for someone to open the email) start
//! the same on-open extraction job the email page uses, and the planner is told they are being read.

use serde_json::{Value, json};
use sqlx::SqlitePool;

use super::{
    command::{Session, ToolError, ToolResult, failed, text_arg},
    datamark,
};
use crate::{attachments, jobs};

/// Characters of one file handed to the planner per call, and in total per request.
pub const READ_CHARS: usize = 6_000;
pub const READ_TOTAL_CHARS: usize = 12_000;
/// Less than this of the budget left is not worth a call.
const MIN_READ_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttState {
    Ready,
    /// Not read yet, or waiting for the on-open job.
    Reading,
    /// Read, but no text came out.
    Empty,
    Failed(String),
    Unreadable(String),
}

impl AttState {
    pub fn label(&self) -> String {
        match self {
            AttState::Ready => "read, text available".into(),
            AttState::Reading => "being read now".into(),
            AttState::Empty => "read, but it has no text".into(),
            AttState::Failed(note) => format!("could not be read ({})", datamark(note, 12)),
            AttState::Unreadable(note) => format!("not readable ({})", datamark(note, 12)),
        }
    }
}

#[derive(sqlx::FromRow)]
pub struct AttRow {
    pub id: String,
    pub message_id: String,
    pub filename: String,
    pub media_type: Option<String>,
    pub size_bytes: Option<i64>,
    pub extract_kind: Option<String>,
    pub extract_status: Option<String>,
    pub extract_error: Option<String>,
    pub text_status: String,
    pub has_text: bool,
}

pub struct AttOverview {
    pub id: String,
    pub filename: String,
    pub kind: String,
    pub size_bytes: Option<i64>,
    pub state: AttState,
}

const ROW_SELECT: &str = "SELECT a.id, a.message_id, a.filename, a.media_type, a.size_bytes, a.extract_kind, a.extract_status, a.extract_error,
        a.text_status, (a.text_content IS NOT NULL AND length(trim(a.text_content)) > 0) AS has_text
     FROM attachments a JOIN email_messages m ON m.id = a.message_id";

fn kind_of(row: &AttRow) -> String {
    row.extract_kind.clone().unwrap_or_else(|| attachments::classify(&row.filename, row.media_type.as_deref()).as_str().to_string())
}

fn state_of(row: &AttRow) -> AttState {
    let note = row.extract_error.clone().unwrap_or_default();
    if row.has_text && (row.text_status == "extracted" || row.extract_status.as_deref() == Some("done")) {
        return AttState::Ready;
    }
    match row.extract_status.as_deref() {
        Some("done") => AttState::Empty,
        Some("failed") => AttState::Failed(note),
        Some("skipped") if note.contains("Open this email") => AttState::Reading,
        Some("skipped") => AttState::Unreadable(note),
        _ if row.text_status == "failed" => AttState::Failed("the file could not be read".into()),
        _ => AttState::Reading,
    }
}

/// Attachments of one message in this workspace, with how reading went. Empty when the message is not in the workspace.
pub async fn overview(db: &SqlitePool, ws: &str, message_id: &str, limit: i64) -> sqlx::Result<Vec<AttOverview>> {
    let rows: Vec<AttRow> =
        sqlx::query_as(&format!("{ROW_SELECT} WHERE a.message_id = ?1 AND m.workspace_id = ?2 ORDER BY a.filename LIMIT ?3"))
            .bind(message_id)
            .bind(ws)
            .bind(limit)
            .fetch_all(db)
            .await?;
    Ok(rows
        .iter()
        .map(|r| AttOverview { id: r.id.clone(), filename: r.filename.clone(), kind: kind_of(r), size_bytes: r.size_bytes, state: state_of(r) })
        .collect())
}

/// Names compared without case, `^` markers or surrounding space, so the planner can quote what it saw.
fn norm_name(s: &str) -> String {
    s.replace('^', " ").split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn defang(s: &str) -> String {
    s.replace('<', "‹").replace('>', "›").replace('"', "'")
}

fn fence(name: &str, kind: &str, from: usize, to: usize, total: usize, text: &str) -> String {
    format!(
        "<attachment name=\"{}\" kind=\"{}\" chars=\"{from}-{to} of {total}\">\n{}\n</attachment>",
        defang(name),
        defang(kind),
        defang(text)
    )
}

impl Session<'_> {
    /// Starts the on-open extraction job for a message; a no-op when it already ran.
    async fn start_reading(&self, message_id: &str) -> Result<(), ToolError> {
        let mut conn = self.state.db.acquire().await?;
        jobs::attachment_jobs::enqueue_extract(&mut conn, self.ws, message_id, true).await?;
        self.state.jobs_notify.notify_one();
        Ok(())
    }

    fn message_arg(&self, args: &Value) -> Result<String, ToolError> {
        text_arg(args, "message_id")
            .map(str::to_string)
            .or_else(|| self.viewing_message.clone())
            .ok_or_else(|| failed("message_id is required"))
    }

    pub(super) async fn list_attachments(&mut self, args: &Value) -> ToolResult {
        let message_id = self.message_arg(args)?;
        let items = overview(&self.state.db, self.ws, &message_id, 20).await?;
        if items.is_empty() {
            let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM email_messages WHERE id = ?1 AND workspace_id = ?2)")
                .bind(&message_id)
                .bind(self.ws)
                .fetch_one(&self.state.db)
                .await?;
            return if exists { Ok(json!({ "message_id": message_id, "attachments": [] })) } else { Err(failed("no email with that id")) };
        }
        self.see(&message_id);
        let pending = items.iter().any(|a| a.state == AttState::Reading);
        if pending {
            self.start_reading(&message_id).await?;
        }
        let list: Vec<Value> = items
            .iter()
            .map(|a| {
                json!({
                    "attachment_id": a.id, "name": datamark(&a.filename, 12), "kind": a.kind, "size_bytes": a.size_bytes,
                    "status": a.state.label(),
                })
            })
            .collect();
        let mut out = json!({ "message_id": message_id, "attachments": list });
        if pending {
            out["note"] = json!("Some files are being read now. Tell the person, and suggest asking again in a moment.");
        }
        Ok(out)
    }

    pub(super) async fn read_attachment(&mut self, args: &Value) -> ToolResult {
        let row = self.find_attachment(args).await?;
        let kind = kind_of(&row);
        let name = datamark(&row.filename, 12);
        match state_of(&row) {
            AttState::Ready => {}
            AttState::Reading => {
                self.start_reading(&row.message_id).await?;
                return Ok(json!({
                    "attachment_id": row.id, "name": name, "kind": kind, "status": "being read now",
                    "note": "I started reading this file. It takes a few seconds. Tell the person it is being read and to ask again shortly.",
                }));
            }
            AttState::Empty => return Err(failed("that file was read but contains no text")),
            AttState::Failed(note) => return Err(failed(format!("that file could not be read: {}", datamark(&note, 15)))),
            AttState::Unreadable(note) => return Err(failed(format!("that file cannot be read: {}", datamark(&note, 15)))),
        }
        let left = READ_TOTAL_CHARS.saturating_sub(self.read_chars);
        if left < MIN_READ_CHARS {
            return Err(failed("reading budget for this request is used up; answer from what you have read"));
        }
        let text: Option<String> = sqlx::query_scalar("SELECT text_content FROM attachments WHERE id = ?1").bind(&row.id).fetch_one(&self.state.db).await?;
        let text = text.unwrap_or_default();
        let total = text.chars().count();
        let from = (args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize).min(total);
        let slice: String = text.chars().skip(from).take(READ_CHARS.min(left)).collect();
        let slice = slice.trim_end().to_string();
        let to = from + slice.chars().count();
        self.read_chars += slice.chars().count();
        self.see(&row.message_id);
        if !self.read.iter().any(|r| r.id == row.id) {
            self.read.push(super::command::ReadAttachment { id: row.id.clone(), message_id: row.message_id.clone(), name: row.filename.clone() });
        }
        let mut out = json!({
            "attachment_id": row.id, "message_id": row.message_id, "name": name, "kind": kind,
            "total_chars": total, "returned": format!("{from}-{to}"),
            "untrusted_text": fence(&row.filename, &kind, from, to, total, &slice),
        });
        if to < total {
            out["next_offset"] = json!(to);
            out["note"] = json!("The file continues. Call read_attachment again with next_offset only if you need more.");
        }
        Ok(out)
    }

    async fn find_attachment(&self, args: &Value) -> Result<AttRow, ToolError> {
        if let Some(id) = text_arg(args, "attachment_id") {
            let row: Option<AttRow> = sqlx::query_as(&format!("{ROW_SELECT} WHERE a.id = ?1 AND m.workspace_id = ?2"))
                .bind(id)
                .bind(self.ws)
                .fetch_optional(&self.state.db)
                .await?;
            return row.ok_or_else(|| failed("no attachment with that id"));
        }
        let message_id = self.message_arg(args).map_err(|_| failed("give attachment_id, or message_id and name"))?;
        let rows: Vec<AttRow> = sqlx::query_as(&format!("{ROW_SELECT} WHERE a.message_id = ?1 AND m.workspace_id = ?2 ORDER BY a.filename LIMIT 50"))
            .bind(&message_id)
            .bind(self.ws)
            .fetch_all(&self.state.db)
            .await?;
        let mut rows = rows;
        match text_arg(args, "name").map(norm_name) {
            None if rows.len() == 1 => Ok(rows.remove(0)),
            None => Err(failed("say which file: give name, or use list_attachments first")),
            Some(wanted) => {
                let exact: Vec<usize> = (0..rows.len()).filter(|&i| norm_name(&rows[i].filename) == wanted).collect();
                let hits = if exact.is_empty() { (0..rows.len()).filter(|&i| norm_name(&rows[i].filename).contains(&wanted)).collect() } else { exact };
                match hits.as_slice() {
                    [i] => Ok(rows.swap_remove(*i)),
                    [] => Err(failed("no attachment with that name on this email; use list_attachments")),
                    _ => Err(failed("several attachments match that name; use attachment_id")),
                }
            }
        }
    }
}
