//! Agents that finish work. A **case** is one goal (fill a requirement, resolve an email); each
//! **step** is one durable job where a planner picks one tool (or finishes). Risky tools wait for
//! a person's approval, a critic checks proposals first, and every turn is traced with its cost.

pub mod approvals;
pub mod autonomy;
pub mod case;
pub mod critic;
pub mod persona;
pub mod planner;
pub mod runner;
pub mod tools;
pub mod triggers;

/// Outcomes a case can record (`update_case` / `finish`).
pub const OUTCOMES: &[&str] = &["submitted", "interview", "rejected", "placed", "closed"];

/// Consecutive error turns after which a case fails.
pub const MAX_ERROR_STREAK: i64 = 3;

/// One plain line for people: whitespace collapsed, bounded length.
pub fn sentence(text: &str, max_chars: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max_chars {
        return flat;
    }
    let cut: String = flat.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::sentence;

    #[test]
    fn sentences_are_single_line_and_bounded() {
        assert_eq!(sentence("  Found\n3   candidates ", 50), "Found 3 candidates");
        assert_eq!(sentence("abcdefghij", 5), "abcd…");
    }
}
