use super::*;

/// Overrides for the drafter while the agent writes. `None` fields keep the workspace values.
#[derive(Debug, Clone, Default)]
pub struct DraftStyle {
    pub instructions: Option<String>,
    pub signature: Option<String>,
    /// The model for the agent's own tier; internal only.
    pub model: Option<String>,
    /// The model for a premium call (the agent's tier raised to `best`); internal only.
    pub premium_model: Option<String>,
}

tokio::task_local! {
    static DRAFT_STYLE: DraftStyle;
}

/// Runs `f` (a drafting call) with the agent's style.
pub async fn with_draft_style<F: Future>(style: DraftStyle, f: F) -> F::Output {
    DRAFT_STYLE.scope(style, f).await
}

/// Hook for the drafter: how a reply to this email should sound. A style set for this call
/// (agent cases, previews) wins; otherwise the workspace agent and the mailbox owner's profile.
pub async fn draft_voice(
    db: &SqlitePool,
    config: &Config,
    ws: &str,
    message_id: &str,
    instructions: String,
    signature: Option<String>,
) -> sqlx::Result<(String, Option<String>)> {
    let style = match DRAFT_STYLE.try_with(Clone::clone) {
        Ok(style) => style,
        Err(_) => context_for_message(db, config, ws, message_id).await?.draft_style(config),
    };
    Ok(merge_voice(style, instructions, signature))
}

/// The agent's voice replaces the standard guidance; the workspace's own reply instructions are
/// kept when the persona doesn't already include them. Signature: the style's, else the workspace's.
pub fn merge_voice(style: DraftStyle, workspace_instructions: String, signature: Option<String>) -> (String, Option<String>) {
    let signature = style.signature.or(signature);
    let Some(persona) = style.instructions else { return (workspace_instructions, signature) };
    let team = workspace_instructions.trim();
    if team.is_empty() || team == DEFAULT_REPLY_STYLE || team == crate::platform::packs::NEUTRAL_REPLY_STYLE || persona.contains(team) {
        return (persona, signature);
    }
    (format!("{persona}\n\nTeam reply instructions:\n{team}"), signature)
}

/// Hook for the drafting model: the agent's tier model when one is set for this call. A premium
/// draft uses the `best` tier's model if one is configured, else the drafter's own premium model.
pub fn draft_model_or(model: &str, premium: bool) -> String {
    DRAFT_STYLE
        .try_with(|s| if premium { s.premium_model.clone() } else { s.model.clone() })
        .ok()
        .flatten()
        .unwrap_or_else(|| model.to_string())
}
