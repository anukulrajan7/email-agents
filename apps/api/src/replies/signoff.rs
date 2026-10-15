//! Agents never sign mail as themselves: the mailbox owner's signature is used. Planner instructions
//! that say "signed off as <agent>" are dropped, and a draft that still ends in the agent's name is fixed.

use std::sync::LazyLock;

use regex::Regex;

static SIGN_OFF_INSTRUCTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)[,;]?\s*(?:and\s+)?\b(?:sign(?:ed|ing)?[\s-]*off(?:\s+(?:as|with|by))?|sign(?:ed)?\s+(?:it|this|the\s+(?:email|reply|mail|message))\s+(?:off\s+)?(?:as|with|by)|signed,\s*\w+|signature\s*:)\s*[^.;\n]*[.;]?",
    )
    .expect("valid regex")
});

/// The instruction without any "sign off as <name>" clause.
pub fn strip_sign_off_instruction(text: &str) -> String {
    let cleaned = SIGN_OFF_INSTRUCTION.replace_all(text, "");
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_name_line(line: &str, agent_name: &str) -> bool {
    let bare: String = line.trim().trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    let name = agent_name.trim().to_lowercase();
    !name.is_empty() && bare == name
}

/// Replaces a closing line that is the agent's own name with the owner's signature. Returns whether it changed.
pub fn replace_agent_sign_off(body: &mut String, agent_name: &str, signature: &str) -> bool {
    let lines: Vec<&str> = body.lines().collect();
    let Some(last) = lines.iter().rposition(|l| !l.trim().is_empty()) else { return false };
    if last == 0 || !is_name_line(lines[last], agent_name) {
        return false;
    }
    let mut kept: Vec<&str> = lines[..last].to_vec();
    kept.push(signature.trim());
    *body = kept.join("\n");
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_sign_off_clauses() {
        let t = strip_sign_off_instruction("Say we have no openings, signed off as Aarav. Keep it short.");
        assert!(!t.to_lowercase().contains("aarav") && t.contains("Keep it short"), "{t}");
        assert_eq!(strip_sign_off_instruction("Confirm receipt"), "Confirm receipt");
        for keep in ["Ask them to sign with DocuSign.", "The contract was signed by the CEO.", "Remind them to sign the form."] {
            assert_eq!(strip_sign_off_instruction(keep), keep, "{keep}");
        }
        assert!(!strip_sign_off_instruction("Thank them. Sign it as Aarav.").contains("Aarav"));
    }

    #[test]
    fn swaps_the_agent_name_for_the_owner() {
        let mut b = "Hello,\n\nThanks.\n\nBest regards,\nAarav".to_string();
        assert!(replace_agent_sign_off(&mut b, "Aarav", "Priya, HR"));
        assert!(b.ends_with("Best regards,\nPriya, HR") && !b.contains("Aarav"), "{b}");
        let mut ok = "Hi,\n\nBest,\nPriya".to_string();
        assert!(!replace_agent_sign_off(&mut ok, "Aarav", "Priya"));
        let mut owner = "Hi,\n\nBest,\nMeera Sharma".to_string();
        assert!(!replace_agent_sign_off(&mut owner, "Meera", "Priya"), "an owner's full name is not the agent's name");
        let mut same = "Hi,\n\nBest,\nMeera".to_string();
        assert!(replace_agent_sign_off(&mut same, "Meera", "Meera"));
        assert!(same.ends_with("Best,\nMeera"));
        let mut mention = "Hi,\n\nThanks.\n\nHR team, with help from Aarav".to_string();
        assert!(!replace_agent_sign_off(&mut mention, "Aarav", "Priya"), "only a line that is just the name");
    }
}
