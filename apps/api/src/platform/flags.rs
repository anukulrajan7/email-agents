//! Workspace flags: the one place a behaviour switch is read or set.
//!
//! Each flag is `off` (today's behaviour), `compare` (the new behaviour runs beside today's and only logs the
//! difference) or `on` (the new behaviour decides). A workspace without a row gets the default from
//! `Config::flag_defaults`, which is empty in production, so everything starts off.

use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::SqlitePool;

use super::{
    audit::{self, Actor, Entry},
    config::Config,
    db::now_ms,
    error::AppResult,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    Handoff,
    GrantsV1,
    Supervisor,
    EarnedTrust,
    HandoffAll,
    OwnerRules,
    AssistantV2,
    VoiceV2,
    FilesV2,
    AssistantRouting,
    Relations,
}

impl Key {
    pub const ALL: [Key; 11] = [
        Key::Handoff,
        Key::GrantsV1,
        Key::Supervisor,
        Key::EarnedTrust,
        Key::HandoffAll,
        Key::OwnerRules,
        Key::AssistantV2,
        Key::VoiceV2,
        Key::FilesV2,
        Key::AssistantRouting,
        Key::Relations,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Key::Handoff => "handoff",
            Key::GrantsV1 => "grants_v1",
            Key::Supervisor => "supervisor",
            Key::EarnedTrust => "earned_trust",
            Key::HandoffAll => "handoff_all",
            Key::OwnerRules => "owner_rules",
            Key::AssistantV2 => "assistant_v2",
            Key::VoiceV2 => "voice_v2",
            Key::FilesV2 => "files_v2",
            Key::AssistantRouting => "assistant_routing",
            Key::Relations => "relations",
        }
    }

    pub fn parse(raw: &str) -> Option<Key> {
        Key::ALL.into_iter().find(|k| k.as_str() == raw)
    }

    /// One plain-English line for the admin screen.
    pub fn description(self) -> &'static str {
        match self {
            Key::Handoff => "Timesheet desk: one owner for each timesheet email, with rules, identity checks, reply duty and a loop for missing data.",
            Key::GrantsV1 => "One-time backfill of agent permissions. A marker, not a behaviour switch.",
            Key::Supervisor => "A supervisor checks each agent proposal, and its verdict decides what is sent or run.",
            Key::EarnedTrust => "Agents that keep getting approved are promoted to more autonomy by themselves.",
            Key::HandoffAll => "Every expert agent works under the same owner contract, with continuation, waiting and merging of related mail.",
            Key::OwnerRules => "Owner-written rules, which also replace the older automations.",
            Key::AssistantV2 => "The assistant reads more of your workspace, remembers the conversation and answers in a plainer style.",
            Key::VoiceV2 => "Spoken answers in a spoken style, one conversation shared between voice and text.",
            Key::FilesV2 => "Attachments are classified and stored, and the assistant can find files.",
            Key::AssistantRouting => "The assistant works out what you want first and sends simple questions to the faster model.",
            Key::Relations => "Agents can hand work to each other and fall back to a related agent.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Off,
    Compare,
    On,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Compare => "compare",
            Mode::On => "on",
        }
    }

    pub fn parse(raw: &str) -> Option<Mode> {
        match raw {
            "off" => Some(Mode::Off),
            "compare" => Some(Mode::Compare),
            "on" => Some(Mode::On),
            _ => None,
        }
    }

    /// The new behaviour decides. Compare mode does not: it only records what it would have done.
    pub fn acts(self) -> bool {
        self == Mode::On
    }

    /// The new behaviour runs beside today's and logs the difference.
    pub fn compares(self) -> bool {
        self == Mode::Compare
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Flag {
    pub key: Key,
    pub description: &'static str,
    pub mode: Mode,
    pub default: Mode,
}

pub fn default_mode(config: &Config, key: Key) -> Mode {
    config.flag_defaults.iter().find(|(k, _)| *k == key).map_or(Mode::Off, |(_, m)| *m)
}

async fn stored(db: &SqlitePool, ws: &str, key: Key) -> sqlx::Result<Option<Mode>> {
    let mode: Option<String> =
        sqlx::query_scalar("SELECT mode FROM workspace_flags WHERE workspace_id = ?1 AND key = ?2").bind(ws).bind(key.as_str()).fetch_optional(db).await?;
    Ok(mode.as_deref().and_then(Mode::parse))
}

pub async fn mode(db: &SqlitePool, config: &Config, ws: &str, key: Key) -> sqlx::Result<Mode> {
    Ok(stored(db, ws, key).await?.unwrap_or_else(|| default_mode(config, key)))
}

/// Every registered flag with its effective mode for the workspace.
pub async fn list(db: &SqlitePool, config: &Config, ws: &str) -> sqlx::Result<Vec<Flag>> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT key, mode FROM workspace_flags WHERE workspace_id = ?1").bind(ws).fetch_all(db).await?;
    Ok(Key::ALL
        .into_iter()
        .map(|key| {
            let default = default_mode(config, key);
            let own = rows.iter().find(|(k, _)| k == key.as_str()).and_then(|(_, m)| Mode::parse(m));
            Flag { key, description: key.description(), mode: own.unwrap_or(default), default }
        })
        .collect())
}

/// Sets a flag for the workspace. A change to the effective mode is audited with who and from → to.
/// The caller has already checked that `user_id` is an admin of `ws`.
pub async fn set(db: &SqlitePool, config: &Config, ws: &str, user_id: &str, key: Key, to: Mode) -> AppResult<Flag> {
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    let from: Mode = {
        let own: Option<String> = sqlx::query_scalar("SELECT mode FROM workspace_flags WHERE workspace_id = ?1 AND key = ?2").bind(ws).bind(key.as_str()).fetch_optional(&mut *tx).await?;
        own.as_deref().and_then(Mode::parse).unwrap_or_else(|| default_mode(config, key))
    };
    sqlx::query(
        "INSERT INTO workspace_flags (workspace_id, key, mode, updated_by, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(workspace_id, key) DO UPDATE SET mode = excluded.mode, updated_by = excluded.updated_by, updated_at = excluded.updated_at",
    )
    .bind(ws)
    .bind(key.as_str())
    .bind(to.as_str())
    .bind(user_id)
    .bind(now_ms())
    .execute(&mut *tx)
    .await?;
    if from != to {
        audit::record(
            &mut tx,
            Entry {
                workspace_id: ws,
                actor: Actor::User(user_id),
                action: "flag.changed",
                entity_type: "workspace_flag",
                entity_id: key.as_str(),
                message_id: None,
                metadata: json!({ "key": key.as_str(), "from": from.as_str(), "to": to.as_str() }),
            },
        )
        .await?;
    }
    if key == Key::Handoff && to.acts() && from != to {
        crate::jobs::enqueue(&mut tx, crate::jobs::kind::REFRESH_IDENTITIES, &format!("refresh_identities:{ws}:{}", now_ms()), Some(ws), &json!({ "workspace_id": ws })).await?;
    }
    tx.commit().await?;
    Ok(Flag { key, description: key.description(), mode: to, default: default_mode(config, key) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(defaults: Vec<(Key, Mode)>) -> Config {
        let mut c = Config::for_tests("sqlite::memory:");
        c.flag_defaults = defaults;
        c
    }

    #[test]
    fn an_unknown_key_is_rejected() {
        assert_eq!(Key::parse("handoff"), Some(Key::Handoff));
        assert_eq!(Key::parse("not_a_flag"), None);
        assert_eq!(Key::parse("Handoff"), None);
        assert_eq!(Key::parse(""), None);
        assert!(serde_json::from_str::<Key>("\"not_a_flag\"").is_err());
    }

    #[test]
    fn every_key_round_trips_and_has_a_description() {
        for key in Key::ALL {
            assert_eq!(Key::parse(key.as_str()), Some(key));
            assert_eq!(serde_json::to_string(&key).unwrap(), format!("\"{}\"", key.as_str()));
            assert!(!key.description().is_empty());
        }
    }

    #[test]
    fn the_default_is_off_unless_config_says_otherwise() {
        assert_eq!(default_mode(&config(vec![]), Key::Supervisor), Mode::Off);
        let c = config(vec![(Key::Supervisor, Mode::Compare)]);
        assert_eq!(default_mode(&c, Key::Supervisor), Mode::Compare);
        assert_eq!(default_mode(&c, Key::Handoff), Mode::Off);
    }

    #[test]
    fn compare_is_distinct_from_on() {
        assert_ne!(Mode::Compare, Mode::On);
        assert!(Mode::On.acts() && !Mode::Compare.acts() && !Mode::Off.acts());
        assert!(Mode::Compare.compares() && !Mode::On.compares() && !Mode::Off.compares());
        assert_eq!(Mode::parse("compare"), Some(Mode::Compare));
        assert_eq!(Mode::parse("shadow"), None);
    }
}
