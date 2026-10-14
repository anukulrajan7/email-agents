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
    agent_profiles::{self, AgentContext, Owner, WorkspaceAgent},
    agents::{
        autonomy::{self, Effective, LevelInputs},
        case::Case,
    },
    config::Config,
    packs,
};

/// Tools that need the HR & recruiting pack's matching feature.
pub const MATCHING_TOOLS: &[&str] = &["find_candidates", "submit_candidate"];

#[derive(Debug, Clone)]
pub struct CaseAgent {
    pub row: AgentRow,
    pub spec: AgentSpec,
    pub version: i64,
    pub level: Effective,
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
    let level = autonomy::effective_level(LevelInputs {
        earned: row.autonomy_level,
        cap: row.autonomy_cap,
        spec_max: i64::from(spec.permissions.max_autonomy),
        env_max: config.agent_max_autonomy,
        profile_asks_first: !owner.is_some_and(Owner::routine_ok),
        active: row.status == "active",
    });
    Ok(CaseAgent { row, spec, version, level })
}

fn playbook(spec: &AgentSpec, persona: &str, personal: bool) -> String {
    let role = if personal { "the mailbox owner's own assistant" } else { spec.name.as_str() };
    let mut out = format!("Your role: {role}. {}\nYour goal: {}\n", spec.description.trim(), spec.goal.trim());
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
    if personal {
        out.push_str("\nYou are working in the mailbox owner's own mail. Write in their voice and follow their profile.\n");
    }
    if persona.trim().is_empty() {
        return out;
    }
    format!("{}\n\n{out}", persona.trim())
}

/// The workspace agent as this agent: its own name, look, rules and permissions, inside the
/// workspace's permission ceiling (sending off, or routine sends off, applies to every agent).
pub fn adapt(mut ctx: AgentContext, agent: &CaseAgent) -> AgentContext {
    if agent.is_lead() {
        return ctx;
    }
    let ws = &ctx.agent;
    let send = if ws.permissions.send == "never" || agent.spec.permissions.send == SendMode::Never {
        "never"
    } else if ws.permissions.send == "auto" && agent.spec.permissions.send == SendMode::Earned {
        "auto"
    } else {
        "ask"
    };
    let quality = match agent.spec.drafting.quality_tier.as_str() {
        "balanced" => ws.quality.clone(),
        tier => tier.to_string(),
    };
    let personal = agent.row.is_personal();
    let (name, avatar, color) =
        if personal { (ws.name.clone(), ws.avatar.clone(), ws.color.clone()) } else { (agent.row.name.clone(), agent.row.avatar.clone(), agent.row.color.clone()) };
    ctx.agent = WorkspaceAgent {
        name,
        avatar,
        color,
        persona: playbook(&agent.spec, &ws.persona, personal),
        permissions: agent_profiles::Permissions { draft: ws.permissions.draft && agent.spec.permissions.draft, send: send.into() },
        quality,
        enabled: ws.enabled,
        ..ws.clone()
    };
    ctx
}

/// The context a case runs in (voice, owner, permissions) and the agent behind it.
pub async fn context_for_case(db: &SqlitePool, config: &Config, case: &Case) -> sqlx::Result<(AgentContext, CaseAgent)> {
    let ctx = agent_profiles::context_for_case(db, config, case).await?;
    let agent = load(db, config, case, ctx.owner.as_ref()).await?;
    Ok((adapt(ctx, &agent), agent))
}

/// Does this workspace have the matching feature (tools `find_candidates`, `submit_candidate`)?
pub async fn matching_enabled(db: &SqlitePool, ws: &str) -> sqlx::Result<bool> {
    packs::has_feature(db, ws, packs::FEATURE_MATCHING).await
}
