//! Per-workspace reply settings: whether drafts are written automatically, how replies
//! should sound, and the sign-off. Written by the team, so trusted in the drafting prompt.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::{config::Config, db::now_ms};

pub const MAX_INSTRUCTIONS_CHARS: usize = 2_000;
pub const MAX_SIGNATURE_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplySettings {
    pub auto_reply: bool,
    #[serde(default)]
    pub reply_instructions: String,
    #[serde(default)]
    pub signature: Option<String>,
}

impl ReplySettings {
    /// Trims, drops an empty signature and enforces the length limits.
    pub fn validated(self) -> Result<Self, String> {
        let reply_instructions = self.reply_instructions.trim().to_string();
        if reply_instructions.chars().count() > MAX_INSTRUCTIONS_CHARS {
            return Err(format!("reply instructions can be at most {MAX_INSTRUCTIONS_CHARS} characters"));
        }
        let signature = self.signature.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        if signature.as_ref().is_some_and(|s| s.chars().count() > MAX_SIGNATURE_CHARS) {
            return Err(format!("signature can be at most {MAX_SIGNATURE_CHARS} characters"));
        }
        Ok(Self { auto_reply: self.auto_reply, reply_instructions, signature })
    }
}

pub async fn load(pool: &SqlitePool, config: &Config, workspace_id: &str) -> sqlx::Result<ReplySettings> {
    let row: Option<(Option<bool>, String, Option<String>)> = sqlx::query_as(
        "SELECT auto_reply, reply_instructions, signature FROM workspace_reply_settings WHERE workspace_id = ?1",
    )
    .bind(workspace_id)
    .fetch_optional(pool)
    .await?;
    let (auto_reply, reply_instructions, signature) = row.unwrap_or((None, String::new(), None));
    Ok(ReplySettings {
        auto_reply: auto_reply.unwrap_or(config.auto_drafts),
        reply_instructions,
        signature: signature.filter(|s| !s.trim().is_empty()),
    })
}

pub async fn save(pool: &SqlitePool, workspace_id: &str, s: &ReplySettings) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO workspace_reply_settings (workspace_id, auto_reply, reply_instructions, signature, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(workspace_id) DO UPDATE SET
           auto_reply = excluded.auto_reply, reply_instructions = excluded.reply_instructions,
           signature = excluded.signature, updated_at = excluded.updated_at",
    )
    .bind(workspace_id)
    .bind(s.auto_reply)
    .bind(&s.reply_instructions)
    .bind(&s.signature)
    .bind(now_ms())
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(instructions: &str, signature: Option<&str>) -> ReplySettings {
        ReplySettings { auto_reply: false, reply_instructions: instructions.into(), signature: signature.map(str::to_string) }
    }

    #[test]
    fn trims_and_drops_blank_signature() {
        let v = settings("  Be brief.  ", Some("   ")).validated().unwrap();
        assert_eq!((v.reply_instructions.as_str(), v.signature), ("Be brief.", None));
    }

    #[test]
    fn enforces_limits() {
        assert!(settings(&"x".repeat(MAX_INSTRUCTIONS_CHARS), None).validated().is_ok());
        assert!(settings(&"x".repeat(MAX_INSTRUCTIONS_CHARS + 1), None).validated().is_err());
        assert!(settings("", Some(&"x".repeat(MAX_SIGNATURE_CHARS + 1))).validated().is_err());
    }
}
