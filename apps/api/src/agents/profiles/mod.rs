//! The workspace's agent and each person's profile. There is one agent per workspace (name,
//! persona, jobs, permissions, quality tier); it adapts to whoever owns the mailbox an email
//! arrived in by reading that person's profile. Both are team-written, so they are trusted in
//! prompts, but they always come after the fixed safety rules, which win.

use std::future::Future;

use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::{
    ai::routing::DEFAULT_REPLY_STYLE,
    platform::audit::{self, Actor, Entry},
    platform::config::Config,
    platform::db::{iso, now_ms},
    mail::sources::normalize::truncate_chars,
};

mod workspace;
mod user;
mod context;
mod style;
mod input;

pub use workspace::Permissions;
pub use workspace::WorkspaceAgent;
pub use workspace::effective_tier;
pub use workspace::tier_model;
pub use workspace::default_persona;
pub use workspace::standard_guidance;
pub use workspace::agent;
pub use workspace::save_agent;
pub use user::HANDLES;
pub use user::AUDIENCES;
pub use user::FORMALITIES;
pub use user::LENGTHS;
pub use user::SEND_APPROVALS;
pub use user::MAX_VIPS;
pub use user::Tone;
pub use user::ProfileApprovals;
pub use user::UserProfile;
pub use user::user_profile;
pub use user::save_user_profile;
pub use user::skip_user_profile;
pub use context::Owner;
pub use context::AgentContext;
pub use context::context_for_message;
pub use context::context_for_case;
pub use style::DraftStyle;
pub use style::with_draft_style;
pub use style::draft_voice;
pub use style::merge_voice;
pub use style::draft_model_or;
pub use input::AgentInput;
pub use input::PermissionsInput;
pub use input::AgentFields;
pub use input::UserProfileInput;
pub use input::ToneInput;
pub use input::ApprovalsInput;
pub use input::valid_avatar;
use input::parse_list;

pub const MAX_NAME_CHARS: usize = 60;
pub const MAX_PERSONA_CHARS: usize = 3_000;
pub const MAX_SIGNATURE_CHARS: usize = 200;
pub const TASK_REPLY: &str = "reply";
pub const TASK_REQUIREMENTS: &str = "requirements";
pub const TASKS: &[&str] = &[TASK_REPLY, TASK_REQUIREMENTS];
pub const COLORS: &[&str] = &["slate", "sage", "plum", "ochre", "teal", "postmark", "ink"];
pub const QUALITIES: &[&str] = &["fast", "balanced", "best"];
pub const SEND_MODES: &[&str] = &["never", "ask", "auto"];

pub const DEFAULT_NAME: &str = "Meera";
pub const DEFAULT_AVATAR: &str = "🌼";
pub const DEFAULT_COLOR: &str = "ochre";
pub(crate) const DEFAULT_PERSONALITY: &str = "I'm Meera, the team's email assistant. I'm calm and organised, \
and always professional: friendly greetings, clear next steps, no jargon, and never more words than needed.";

#[cfg(test)]
mod tests {
    use super::*;

    fn agent() -> WorkspaceAgent {
        WorkspaceAgent {
            workspace_id: "ws".into(),
            name: DEFAULT_NAME.into(),
            avatar: DEFAULT_AVATAR.into(),
            color: DEFAULT_COLOR.into(),
            persona: "Warm.".into(),
            signature: Some("Meera, Support Desk".into()),
            tasks: vec!["reply".into(), "requirements".into()],
            permissions: Permissions::default(),
            quality: "balanced".into(),
            enabled: true,
            updated_at: 0,
        }
    }

    fn input<T: for<'de> Deserialize<'de>>(v: Value) -> T {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn avatars_are_one_emoji_or_two_letters() {
        assert_eq!(valid_avatar(" as "), Some("AS".into()));
        assert_eq!(valid_avatar("🌼"), Some("🌼".into()));
        assert_eq!(valid_avatar("👩‍💼"), Some("👩‍💼".into()));
        assert_eq!(valid_avatar("fluent:owl"), Some("fluent:owl".into()));
        assert_eq!(valid_avatar("fluent:nope"), None);
        assert_eq!(valid_avatar("abc"), None);
        assert_eq!(valid_avatar("a!"), None);
        assert_eq!(valid_avatar(""), None);
    }

    #[test]
    fn validates_agent_updates() {
        let a = agent()
            .apply(&input(json!({"name": "  Asha   ", "tasks": ["requirements", "reply"], "permissions": {"send": "auto"}, "signature": null})))
            .unwrap();
        assert_eq!((a.name.as_str(), a.signature.as_deref(), a.permissions.send.as_str()), ("Asha", None, "auto"));
        assert_eq!(a.tasks, ["reply", "requirements"]);
        assert_eq!(agent().apply(&input(json!({}))).unwrap(), agent(), "absent fields are kept");
        let bad = |v: Value| agent().apply(&input(v)).is_err();
        assert!(bad(json!({"name": ""})));
        assert!(bad(json!({"name": "x".repeat(61)})));
        assert!(bad(json!({"persona": "x".repeat(3001)})));
        assert!(bad(json!({"tasks": ["payroll"]})));
        assert!(bad(json!({"color": "pink"})));
        assert!(bad(json!({"quality": "premium"})));
        assert!(bad(json!({"permissions": {"send": "sometimes"}})));
        assert!(bad(json!({"permissions": {"submit_candidate": "auto"}})));
        assert!(serde_json::from_value::<AgentInput>(json!({"model": "x"})).is_err());
    }

    #[test]
    fn validates_user_profiles() {
        let p = UserProfile::blank()
            .apply(&input(json!({"display_name": "Priya", "job_title": "Recruiter", "handles": ["candidates", "leave"],
                                 "audiences": ["clients"], "vips": ["CEO@Client.com", "@client.com", "client.com"],
                                 "tone": {"formality": "friendly", "length": "short", "sign_off": " Priya ", "language": "English"},
                                 "approvals": {"send_email": "routine_ok"}})))
            .unwrap();
        assert_eq!(p.handles, ["candidates", "leave"]);
        assert_eq!(p.vips, ["ceo@client.com", "client.com"]);
        assert_eq!((p.tone.formality.as_str(), p.tone.sign_off.as_deref()), ("friendly", Some("Priya")));
        assert!(p.routine_ok(), "an unsaved profile has not asked to be asked first");
        assert!(UserProfile { saved: true, ..p.clone() }.routine_ok());
        assert!(!UserProfile { saved: true, approvals: ProfileApprovals { send_email: "always_ask".into() }, ..p.clone() }.routine_ok());
        let cleared = p.apply(&input(json!({"tone": {"sign_off": null}}))).unwrap();
        assert_eq!(cleared.tone.sign_off, None);
        let bad = |v: Value| UserProfile::blank().apply(&input(v)).is_err();
        assert!(bad(json!({"handles": ["hacking"]})));
        assert!(bad(json!({"tone": {"formality": "angry"}})));
        assert!(bad(json!({"tone": {"length": "epic"}})));
        assert!(bad(json!({"approvals": {"send_email": "never"}})));
        assert!(bad(json!({"approvals": {"submit_candidate": "routine_ok"}})));
        assert!(bad(json!({"vips": ["not an address"]})));
        assert!(bad(json!({"vips": (0..21).map(|i| format!("p{i}@x.com")).collect::<Vec<_>>()})));
        assert!(bad(json!({"never_do": "x".repeat(1001)})));
    }

    #[test]
    fn tiers_combine_with_one_rule() {
        for (agent, premium, want) in [
            ("fast", false, "fast"), ("fast", true, "best"), ("balanced", false, "balanced"), ("balanced", true, "best"),
            ("best", false, "best"), ("best", true, "best"), ("anything else", false, "balanced"),
        ] {
            assert_eq!(effective_tier(agent, premium), want, "{agent} premium={premium}");
        }
        let mut config = Config::for_tests("sqlite::memory:");
        config.agent_fast_model_id = "fast-m".into();
        config.agent_best_model_id = "best-m".into();
        let ctx = |quality: &str| AgentContext {
            agent: WorkspaceAgent { quality: quality.into(), ..agent() },
            mailbox: None,
            owner: None,
        };
        let style = ctx("fast").draft_style(&config);
        assert_eq!((style.model.as_deref(), style.premium_model.as_deref()), (Some("fast-m"), Some("best-m")), "premium raises the floor");
        let style = ctx("balanced").draft_style(&config);
        assert_eq!((style.model.as_deref(), style.premium_model.as_deref()), (None, Some("best-m")), "no balanced model configured");
        let style = ctx("best").draft_style(&config);
        assert_eq!((style.model.as_deref(), style.premium_model.as_deref()), (Some("best-m"), Some("best-m")));
    }

    #[test]
    fn stored_permissions_fall_back_to_asking() {
        assert_eq!(Permissions::parse("garbage"), Permissions::default());
        assert_eq!(Permissions::parse(r#"{"draft":false,"send":"auto"}"#), Permissions { draft: false, send: "auto".into() });
        assert_eq!(Permissions::parse(r#"{"send":"yolo"}"#).send, "ask");
    }

    #[test]
    fn seeded_persona_is_friendly_and_adds_workspace_instructions() {
        assert!(standard_guidance(DEFAULT_REPLY_STYLE).starts_with("I'm Meera") && standard_guidance(DEFAULT_REPLY_STYLE).contains(DEFAULT_REPLY_STYLE));
        assert!(default_persona(DEFAULT_REPLY_STYLE, "Mention our 1-day turnaround.").ends_with("\n\nMention our 1-day turnaround."));
    }

    #[test]
    fn context_blocks_name_the_agent_and_the_person() {
        let profile = UserProfile { saved: true, ..UserProfile::blank() }
            .apply(&input(json!({"display_name": "Priya", "job_title": "Recruiter", "never_do": "Share salary bands", "tone": {"sign_off": "Priya S."}})))
            .unwrap();
        let ctx = AgentContext {
            agent: agent(),
            mailbox: Some("priya@acme.com".into()),
            owner: Some(Owner { user_id: "u1".into(), email: "priya@acme.com".into(), profile: Some(profile) }),
        };
        let block = ctx.planner_block();
        assert!(block.contains("You are Meera, the team's email agent for the mailbox priya@acme.com."));
        assert!(block.contains("Warm.") && block.contains("You are working for Priya (priya@acme.com), Recruiter."));
        assert!(block.contains("- Never do this for them: Share salary bands"));
        let style = ctx.draft_style(&Config::for_tests("sqlite::memory:"));
        assert_eq!(style.signature.as_deref(), Some("Priya S."), "the person's signature wins");
        assert!(style.instructions.unwrap().contains("Recruiter"));
        assert_eq!(style.model, None, "no tier model configured in tests");
    }

    #[tokio::test]
    async fn draft_style_applies_only_inside_scope() {
        assert_eq!(draft_model_or("m", false), "m");
        let style = DraftStyle { instructions: Some("persona".into()), signature: None, model: Some("tier".into()), premium_model: Some("top".into()) };
        let inside = with_draft_style(style.clone(), async { (draft_model_or("m", false), draft_model_or("m", true)) }).await;
        assert_eq!(inside, ("tier".to_string(), "top".to_string()));
        let no_premium = DraftStyle { premium_model: None, ..style };
        assert_eq!(with_draft_style(no_premium, async { draft_model_or("own premium", true) }).await, "own premium");
        let none = DraftStyle::default();
        assert_eq!(merge_voice(none, "ws".into(), Some("sig".into())), ("ws".to_string(), Some("sig".to_string())));
        let persona = || DraftStyle { instructions: Some("Be Meera.".into()), signature: Some("Meera".into()), model: None, premium_model: None };
        assert_eq!(merge_voice(persona(), DEFAULT_REPLY_STYLE.into(), None), ("Be Meera.".to_string(), Some("Meera".to_string())));
        assert_eq!(merge_voice(persona(), "Mention SLAs.".into(), None).0, "Be Meera.\n\nTeam reply instructions:\nMention SLAs.");
    }
}
