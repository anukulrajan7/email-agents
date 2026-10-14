//! Whose learning this is: the agent, the person it works for, and how much of the team's memory it may use.

use sqlx::SqliteConnection;

use crate::agent_registry::{
    spec::{LearnFrom, MemoryScope},
    store, versions,
};

#[derive(Debug, Clone)]
pub struct AgentScope {
    pub agent_id: Option<String>,
    pub personal: bool,
    /// The person the agent writes for: a personal agent's owner, else whoever owns the mailbox.
    pub owner_user_id: Option<String>,
    pub memory: MemoryScope,
    pub learn_from_edits: bool,
}

impl AgentScope {
    /// Facts this agent learns carry its id only when its memory scope is `own`.
    pub fn fact_agent_id(&self) -> Option<&str> {
        match (self.memory, &self.agent_id) {
            (MemoryScope::Own, Some(id)) => Some(id),
            _ => None,
        }
    }
}

/// The agent a message was routed to.
pub async fn routed_agent(conn: &mut SqliteConnection, message_id: &str) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT agent_id FROM message_routes WHERE message_id = ?1").bind(message_id).fetch_optional(conn).await
}

/// The person who connected the mailbox this message arrived in.
pub async fn mailbox_owner(conn: &mut SqliteConnection, message_id: &str) -> sqlx::Result<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT s.connected_by_user_id FROM email_messages m JOIN email_sources s ON s.id = m.source_id WHERE m.id = ?1",
    )
    .bind(message_id)
    .fetch_optional(conn)
    .await?
    .flatten())
}

/// `agent_id` wins; otherwise the agent the message was routed to; otherwise none (workspace-wide defaults).
pub async fn resolve(conn: &mut SqliteConnection, ws: &str, agent_id: Option<&str>, message_id: Option<&str>) -> sqlx::Result<AgentScope> {
    let agent_id = match (agent_id, message_id) {
        (Some(a), _) => Some(a.to_string()),
        (None, Some(m)) => routed_agent(conn, m).await?,
        (None, None) => None,
    };
    let row = match &agent_id {
        Some(id) => store::get_in(&mut *conn, ws, id).await?,
        None => None,
    };
    let mut scope = AgentScope { agent_id: None, personal: false, owner_user_id: None, memory: MemoryScope::OwnShared, learn_from_edits: true };
    if let Some(row) = row {
        if let Ok((spec, _)) = versions::active_spec(conn, &row).await {
            scope.memory = spec.memory.scope;
            scope.learn_from_edits = spec.memory.learn_from.iter().any(|l| matches!(l, LearnFrom::Edits | LearnFrom::Approvals));
        }
        scope.personal = row.is_personal();
        scope.owner_user_id = row.owner_user_id.clone();
        scope.agent_id = Some(row.id);
    }
    if scope.owner_user_id.is_none()
        && let Some(m) = message_id
    {
        scope.owner_user_id = mailbox_owner(conn, m).await?;
    }
    Ok(scope)
}
