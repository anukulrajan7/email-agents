use super::*;

/// The person whose mailbox an email arrived in, with their profile if they wrote one.
#[derive(Debug, Clone)]
pub struct Owner {
    pub user_id: String,
    pub email: String,
    pub profile: Option<UserProfile>,
}

impl Owner {
    pub fn routine_ok(&self) -> bool {
        self.profile.as_ref().is_none_or(UserProfile::routine_ok)
    }
}

/// Everything the agent needs to know about who it is and who it works for on one email.
#[derive(Debug, Clone)]
pub struct AgentContext {
    pub agent: WorkspaceAgent,
    pub mailbox: Option<String>,
    pub owner: Option<Owner>,
}

impl AgentContext {
    /// The trusted prompt blocks (agent persona + the owner's profile) for planner and drafter.
    pub fn blocks(&self) -> crate::agents::persona::PromptBlocks {
        crate::agents::persona::build_prompt_blocks(&self.agent, self.mailbox.as_deref(), self.owner.as_ref(), None)
    }

    /// Trusted block for the planner, placed after the safety rules.
    pub fn planner_block(&self) -> String {
        self.blocks().planner
    }

    /// How drafts for this email should sound: the agent's persona plus the owner's profile,
    /// the owner's signature (never the agent's own) and the quality tier's model.
    pub fn draft_style(&self, config: &Config) -> DraftStyle {
        let signature = self.owner.as_ref().and_then(|o| o.profile.as_ref()).and_then(|p| p.tone.sign_off.clone());
        let model = Some(tier_model(config, effective_tier(&self.agent.quality, false)).to_string()).filter(|m| !m.is_empty());
        let premium_model = Some(tier_model(config, effective_tier(&self.agent.quality, true)).to_string()).filter(|m| !m.is_empty());
        DraftStyle { instructions: Some(self.blocks().drafter).filter(|i| !i.trim().is_empty()), signature, model, premium_model }
    }
}

#[derive(sqlx::FromRow)]
struct MailboxRow {
    mailbox: Option<String>,
    owner_id: Option<String>,
    owner_email: Option<String>,
}

/// The agent, the mailbox and its owner for one email of this workspace.
pub async fn context_for_message(db: &SqlitePool, config: &Config, ws: &str, message_id: &str) -> sqlx::Result<AgentContext> {
    let agent = agent(db, config, ws).await?;
    let row: Option<MailboxRow> = sqlx::query_as(
        "SELECT COALESCE(s.mailbox_address, s.inbound_alias) AS mailbox, u.id AS owner_id, u.email AS owner_email
         FROM email_messages m JOIN email_sources s ON s.id = m.source_id
         LEFT JOIN users u ON u.id = s.connected_by_user_id
           AND EXISTS (SELECT 1 FROM memberships ms WHERE ms.workspace_id = m.workspace_id AND ms.user_id = u.id)
         WHERE m.id = ?1 AND m.workspace_id = ?2",
    )
    .bind(message_id)
    .bind(ws)
    .fetch_optional(db)
    .await?;
    let Some(row) = row else { return Ok(AgentContext { agent, mailbox: None, owner: None }) };
    let owner = match (row.owner_id, row.owner_email) {
        (Some(user_id), Some(email)) => Some(Owner { profile: user_profile(db, ws, &user_id).await?, user_id, email }),
        _ => None,
    };
    Ok(AgentContext { agent, mailbox: row.mailbox.filter(|m| !m.trim().is_empty()), owner })
}

/// The agent and the person it works for on a case (from the case's email).
pub async fn context_for_case(db: &SqlitePool, config: &Config, case: &crate::agents::case::Case) -> sqlx::Result<AgentContext> {
    match case.message_id.as_deref() {
        Some(m) => context_for_message(db, config, &case.workspace_id, m).await,
        None => Ok(AgentContext { agent: agent(db, config, &case.workspace_id).await?, mailbox: None, owner: None }),
    }
}
