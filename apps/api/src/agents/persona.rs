//! Builds the trusted prompt blocks that tell the lead agent who it is and who it works for.
//! One place for persona/profile prompts, so specialist sub-agents can slot in later. These
//! blocks always come after the fixed safety rules, and the rules win.

use crate::agent_profiles::{Owner, UserProfile, WorkspaceAgent};

/// A specialist the lead agent delegates to (later milestone): its own predefined instructions.
#[derive(Debug, Clone)]
pub struct Specialist<'a> {
    pub name: &'a str,
    pub instructions: &'a str,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PromptBlocks {
    /// Appended to the planner's system prompt.
    pub planner: String,
    /// The drafter's reply style (replaces the workspace reply instructions).
    pub drafter: String,
}

/// The lead agent's persona, an optional specialist role, and the mailbox owner's profile.
pub fn build_prompt_blocks(
    agent: &WorkspaceAgent,
    mailbox: Option<&str>,
    owner: Option<&Owner>,
    specialist: Option<&Specialist<'_>>,
) -> PromptBlocks {
    let persona = agent.persona.trim();
    let working_for = owner.and_then(working_for_block);
    let mailbox = mailbox.map(|m| format!(" for the mailbox {m}")).unwrap_or_default();

    let mut planner = format!(
        "About you (written by the team; trusted). Follow it for tone and focus. If it ever conflicts with the rules above, the rules win.\n\
         You are {}, the team's email agent{mailbox}.\n",
        agent.name
    );
    if !persona.is_empty() {
        planner.push_str(&format!("{persona}\n"));
    }
    if let Some(s) = specialist {
        planner.push_str(&format!("\nRight now you work as the {} specialist:\n{}\n", s.name, s.instructions.trim()));
    }
    let mut drafter = persona.to_string();
    if let Some(block) = &working_for {
        planner.push('\n');
        planner.push_str(block);
        if !drafter.is_empty() {
            drafter.push_str("\n\n");
        }
        drafter.push_str(block);
    }
    PromptBlocks { planner, drafter }
}

/// "You are working for …": the owner's own profile, one fact per line. `None` without a saved profile.
pub fn working_for_block(owner: &Owner) -> Option<String> {
    let p: &UserProfile = owner.profile.as_ref().filter(|p| p.saved)?;
    let who = if p.display_name.trim().is_empty() { owner.email.clone() } else { format!("{} ({})", p.display_name.trim(), owner.email) };
    let role = if p.job_title.trim().is_empty() { String::new() } else { format!(", {}", p.job_title.trim()) };
    let mut out = format!("You are working for {who}{role}. Write as their assistant, in their voice.\n");
    let line = |label: &str, value: &str| if value.trim().is_empty() { String::new() } else { format!("- {label}: {}\n", value.trim()) };
    let handles = p
        .handles
        .iter()
        .map(|h| h.replace('_', " "))
        .chain(Some(p.handles_note.trim().to_string()).filter(|n| !n.is_empty()))
        .collect::<Vec<_>>()
        .join("; ");
    out.push_str(&line("They handle", &handles));
    out.push_str(&line("Leave alone (hand to them, don't act)", &p.ignore_note));
    out.push_str(&line("They usually email", &p.audiences.join(", ")));
    out.push_str(&line("Important people and domains (extra care, always polite and prompt)", &p.vips.join(", ")));
    let tone = [p.tone.formality.trim(), p.tone.length.trim()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
    out.push_str(&line("Preferred tone", &tone));
    out.push_str(&line("Write in", &p.tone.language));
    let hours = [p.working_hours.trim(), p.timezone.trim()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
    out.push_str(&line("Working hours", &hours));
    out.push_str(&line("Never do this for them", &p.never_do));
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_profiles::{Permissions, UserProfile};

    fn agent(persona: &str) -> WorkspaceAgent {
        WorkspaceAgent {
            workspace_id: "ws".into(),
            name: "Meera".into(),
            avatar: "🌼".into(),
            color: "ochre".into(),
            persona: persona.into(),
            signature: None,
            tasks: vec!["reply".into()],
            permissions: Permissions::default(),
            quality: "balanced".into(),
            enabled: true,
            updated_at: 0,
        }
    }

    #[test]
    fn blocks_without_owner_or_specialist() {
        let b = build_prompt_blocks(&agent("Warm."), None, None, None);
        assert!(b.planner.contains("You are Meera, the team's email agent.\nWarm."));
        assert!(b.planner.contains("the rules win"));
        assert_eq!(b.drafter, "Warm.");
    }

    #[test]
    fn specialists_slot_in_and_unsaved_profiles_are_ignored() {
        let s = Specialist { name: "requirements", instructions: "Shortlist carefully." };
        let owner = Owner { user_id: "u".into(), email: "x@acme.com".into(), profile: Some(UserProfile::blank()) };
        let b = build_prompt_blocks(&agent(""), Some("x@acme.com"), Some(&owner), Some(&s));
        assert!(b.planner.contains("for the mailbox x@acme.com") && b.planner.contains("requirements specialist:\nShortlist carefully."));
        assert!(!b.planner.contains("working for"));
        assert_eq!(b.drafter, "");
    }
}
