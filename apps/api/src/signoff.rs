//! Agents never sign mail as themselves: the mailbox owner's signature is used. Planner instructions
//! that say "signed off as <agent>" are dropped, and a draft that still ends in the agent's name is fixed.

use std::sync::LazyLock;

use regex::Regex;

static SIGN_OFF_INSTRUCTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[,;]?\s*(?:and\s+)?\b(?:sign(?:ed|ing)?[\s-]*(?:it\s+)?(?:off\s+)?(?:as|by|with|from)|signature\s*:)\s*[^.;\n]*[.;]?")
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
    !name.is_empty() && (bare == name || bare.ends_with(&format!(" {name}")) && bare.split_whitespace().count() <= 4)
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
    }

    #[test]
    fn swaps_the_agent_name_for_the_owner() {
        let mut b = "Hello,\n\nThanks.\n\nBest regards,\nAarav".to_string();
        assert!(replace_agent_sign_off(&mut b, "Aarav", "Priya, HR"));
        assert!(b.ends_with("Best regards,\nPriya, HR") && !b.contains("Aarav"), "{b}");
        let mut ok = "Hi,\n\nBest,\nPriya".to_string();
        assert!(!replace_agent_sign_off(&mut ok, "Aarav", "Priya"));
    }
}
