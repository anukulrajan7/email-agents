//! Which agent works a case, at what autonomy level, with which tools and voice. The lead keeps
//! the workspace agent's persona and permissions as they are today; every other agent is the same
//! workspace agent narrowed by its own spec (the workspace settings are a ceiling for everyone).

use sqlx::SqlitePool;

use super::{
    spec::{AgentSpec, Mode, SendMode},
    store::{self, AgentRow},
    versions,
};
use crate::{
    agents::profiles::{self, AgentContext, Owner, WorkspaceAgent},
    agents::{
        autonomy::{self, Effective, LevelInputs},
        case::Case,
    },
    platform::config::Config,
    platform::packs,
};

/// Tools that need the HR & recruiting pack's matching feature.
pub const MATCHING_TOOLS: &[&str] = &["find_candidates", "find_requirements", "submit_candidate"];

#[derive(Debug, Clone)]
pub struct CaseAgent {
    pub row: AgentRow,
    pub spec: AgentSpec,
    pub version: i64,
    pub level: Effective,
    /// The agent's own level, before a reply job's governing level replaced `level`.
    pub own_level: i64,
}

impl CaseAgent {
    pub fn is_lead(&self) -> bool {
        self.row.kind == "lead"
    }

    pub fn is_active(&self) -> bool {
        self.row.status == "active"
    }

    /// Tools this agent may call: its spec's list that its scopes cover, without matching tools when the pack is off.
    pub fn allowed_tools(&self, matching: bool) -> Vec<String> {
        self.spec.usable_tools().into_iter().filter(|t| matching || !MATCHING_TOOLS.contains(&t.as_str())).collect()
    }
}

/// The agent that works this case. Cases from before the registry (no `agent_id`) belong to the lead.
pub async fn load(db: &SqlitePool, config: &Config, case: &Case, owner: Option<&Owner>) -> sqlx::Result<CaseAgent> {
    let mut row = match case.agent_id.as_deref() {
        Some(id) => store::get_in(db, &case.workspace_id, id).await?,
        None => None,
    };
    if row.is_none() {
        row = store::lead(db, &case.workspace_id).await?;
    }
    if row.is_none() {
        super::seed::ensure_workspace(db, &case.workspace_id).await?;
        row = store::lead(db, &case.workspace_id).await?;
    }
    let row = row.ok_or(sqlx::Error::RowNotFound)?;
    let (spec, version) = versions::active_spec_in(db, &row).await?;
    let mut level = level_for(&row, &spec, config, owner);
    let own_level = level.level;
    // Meera's reply job runs at the level of the experts who asked for the words, not at her own.
    if crate::agents::workflow::is_reply_job(case)
        && row.status == "active"
        && let Some(stored) = crate::agents::handoff::store::governing_for_message(db, &case.workspace_id, case.message_id.as_deref()).await?
    {
        // What was captured when the request was filed, lowered by whoever has been demoted or paused since.
        let now = match case.message_id.as_deref() {
            Some(m) => crate::agents::handoff::store::governing_now(db, config, &case.workspace_id, m).await?.unwrap_or(stored),
            None => stored,
        };
        level = Effective { level: stored.min(now), limited_by: "handoff" };
    }
    Ok(CaseAgent { row, spec, version, level, own_level })
}

/// The effective level of an agent working for this mailbox owner (an owner who wants to be asked first caps it at 1).
pub fn level_for(row: &AgentRow, spec: &AgentSpec, config: &Config, owner: Option<&Owner>) -> Effective {
    autonomy::effective_level(LevelInputs {
        earned: row.autonomy_level,
        cap: row.autonomy_cap,
        spec_max: i64::from(spec.permissions.max_autonomy),
        env_max: config.agent_max_autonomy,
        profile_asks_first: !owner.is_some_and(Owner::routine_ok),
        active: row.status == "active",
    })
}

/// One agent's effective level on one email (the mailbox owner's profile counts), as of now.
pub async fn level_of(db: &SqlitePool, config: &Config, ws: &str, agent_id: &str, message_id: &str) -> sqlx::Result<i64> {
    let Some(row) = store::get_in(db, ws, agent_id).await? else { return Ok(0) };
    let (spec, _) = versions::active_spec_in(db, &row).await?;
    let ctx = profiles::context_for_message(db, config, ws, message_id).await?;
    Ok(level_for(&row, &spec, config, ctx.owner.as_ref()).level)
}

fn playbook(spec: &AgentSpec, team_voice: &str, personal: bool) -> String {
    let role = if personal { "the mailbox owner's own assistant" } else { spec.name.as_str() };
    let mut out = format!("Your role: {role}. {}\nYour goal: {}\n", spec.description.trim(), spec.goal.trim());
    if !spec.persona.trim().is_empty() {
        out.push_str(&format!("How you think:\n{}\n", spec.persona.trim()));
    }
    if spec.mode == Mode::Workflow && !spec.steps.is_empty() {
        out.push_str("Work through these steps in order, skipping any that do not apply:\n");
        for (i, s) in spec.steps.iter().enumerate() {
            out.push_str(&format!("{}. {}\n", i + 1, s.describe()));
        }
    }
    if !spec.drafting.rules.is_empty() {
        out.push_str("Writing rules:\n");
        spec.drafting.rules.iter().for_each(|r| out.push_str(&format!("- {r}\n")));
    }
    if !spec.drafting.never_do.is_empty() {
        out.push_str("Never:\n");
        spec.drafting.never_do.iter().for_each(|r| out.push_str(&format!("- {r}\n")));
    }
    out.push_str("\nEmails you write go out in the mailbox owner's voice. Never sign them with your own name.\n");
    if personal {
        out.push_str("You are working in the mailbox owner's own mail. Write in their voice and follow their profile.\n");
        return out;
    }
    if team_voice.trim().is_empty() {
        return out;
    }
    format!("{out}\nTeam voice and rules:\n{}\n", team_voice.trim())
}

/// The send permission an agent really has: the workspace ceiling and the agent's own spec, whichever is stricter.
pub fn effective_send(workspace_send: &str, spec_send: SendMode) -> &'static str {
    if workspace_send == "never" || spec_send == SendMode::Never {
        "never"
    } else if workspace_send == "auto" && spec_send == SendMode::Earned {
        "auto"
    } else {
        "ask"
    }
}

/// The workspace agent as this agent: its own name, look, rules and permissions, inside the
/// workspace's permission ceiling (sending off, or routine sends off, applies to every agent).
pub fn adapt(mut ctx: AgentContext, agent: &CaseAgent) -> AgentContext {
    if agent.is_lead() {
        return ctx;
    }
    let ws = &ctx.agent;
    let send = effective_send(&ws.permissions.send, agent.spec.permissions.send);
    let quality = match agent.spec.drafting.quality_tier.as_str() {
        "balanced" => ws.quality.clone(),
        tier => tier.to_string(),
    };
    let personal = agent.row.is_personal();
    let (name, avatar, color) = (agent.row.name.clone(), agent.row.avatar.clone(), agent.row.color.clone());
    ctx.agent = WorkspaceAgent {
        name,
        avatar,
        color,
        persona: playbook(&agent.spec, &ws.persona, personal),
        permissions: profiles::Permissions { draft: ws.permissions.draft && agent.spec.permissions.draft, send: send.into() },
        quality,
        enabled: ws.enabled,
        ..ws.clone()
    };
    ctx
}

/// The context a case runs in (voice, owner, permissions) and the agent behind it.
pub async fn context_for_case(db: &SqlitePool, config: &Config, case: &Case) -> sqlx::Result<(AgentContext, CaseAgent)> {
    let ctx = profiles::context_for_case(db, config, case).await?;
    let agent = load(db, config, case, ctx.owner.as_ref()).await?;
    Ok((adapt(ctx, &agent), agent))
}

/// Does this workspace have the matching feature (tools `find_candidates`, `submit_candidate`)?
pub async fn matching_enabled(db: &SqlitePool, ws: &str) -> sqlx::Result<bool> {
    packs::has_feature(db, ws, packs::FEATURE_MATCHING).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::registry::templates;

    #[test]
    fn the_persona_reaches_the_playbook_and_the_mail_is_never_signed_by_the_agent() {
        for key in ["support.resolver", "sales.qualifier", "hrms.payroll"] {
            let t = templates::template(key).unwrap();
            let book = playbook(&t.spec(), "Team style: short.", false);
            assert!(book.contains(&format!("You are {}", t.name)), "{key}: {book}");
            assert!(book.contains("How you think:"), "{key}");
            assert!(book.contains("Never sign them with your own name"), "{key}");
            assert!(book.contains("Team voice and rules:\nTeam style: short."), "{key}");
        }
    }

    #[test]
    fn the_personal_agent_uses_its_own_persona_not_the_leads() {
        let t = templates::personal_template();
        let book = playbook(&t.spec(), "I'm Aarav, the team's email assistant.", true);
        assert!(book.contains("You are Meera, the mailbox owner's own assistant"));
        assert!(!book.contains("Aarav"), "the lead's identity stays out: {book}");
        assert!(book.contains("Never sign them with your own name"));
    }
}
