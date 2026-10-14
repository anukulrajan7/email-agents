//! How far a person moved a draft: normalised word-level Levenshtein, and a cheap test for whether
//! the final text states something new (a number or a proper noun) that neither the suggestion nor
//! the email had.

use std::collections::HashSet;

/// Edits smaller than this teach nothing on their own.
pub const TINY_EDIT: f64 = 0.03;

/// 0 = same words, 1 = nothing in common. Whitespace (spaces, line breaks) is ignored.
pub fn normalised(before: &str, after: &str) -> f64 {
    let a: Vec<&str> = before.split_whitespace().collect();
    let b: Vec<&str> = after.split_whitespace().collect();
    let longest = a.len().max(b.len());
    if longest == 0 {
        return 0.0;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, wa) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, wb) in b.iter().enumerate() {
            let cost = usize::from(wa != wb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()] as f64 / longest as f64
}

fn word(token: &str) -> String {
    token.trim_matches(|c: char| !c.is_alphanumeric()).to_string()
}

/// Numbers and proper nouns (capitalised words that don't start a sentence) of a text.
fn specifics(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut sentence_start = true;
    for raw in text.split_whitespace() {
        let w = word(raw);
        if !w.is_empty() {
            let proper_noun = !sentence_start && w.chars().count() >= 3 && w.chars().next().is_some_and(char::is_uppercase);
            if w.chars().any(|c| c.is_ascii_digit()) || proper_noun {
                out.insert(w.to_lowercase());
            }
        }
        sentence_start = raw.ends_with(['.', '!', '?', ':']) || raw.contains('\n');
    }
    out
}

/// Does `approved` state a number or proper noun that is in neither the suggestion nor the email?
pub fn adds_specifics(suggested: &str, approved: &str, email: &str) -> bool {
    let known: HashSet<String> = specifics(suggested)
        .into_iter()
        .chain(specifics(email))
        .chain(email.split_whitespace().map(|w| word(w).to_lowercase()))
        .collect();
    specifics(approved).iter().any(|s| !known.contains(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_text_is_zero_and_a_rewrite_is_one() {
        assert_eq!(normalised("Hello Ravi, thanks.", "Hello Ravi, thanks."), 0.0);
        assert_eq!(normalised("", ""), 0.0);
        assert_eq!(normalised("one two three", "alpha beta gamma"), 1.0);
        assert_eq!(normalised("one two three four", ""), 1.0);
    }

    #[test]
    fn whitespace_is_ignored() {
        assert_eq!(normalised("Hello  Ravi,\n\nthanks.", "Hello Ravi, thanks."), 0.0);
    }

    #[test]
    fn partial_edits_are_between() {
        let d = normalised("Hello Ravi thanks for the note", "Hello Ravi thanks for your note");
        assert!((d - 1.0 / 6.0).abs() < 1e-9, "{d}");
    }

    #[test]
    fn new_numbers_and_names_count_as_specifics() {
        let email = "Can you confirm the timesheet date?";
        let suggested = "Hello Ravi,\n\nThanks, we will confirm.";
        assert!(!adds_specifics(suggested, suggested, email));
        assert!(adds_specifics(suggested, "Hello Ravi,\n\nThanks, we ship in 3 days.", email));
        assert!(adds_specifics(suggested, "Hello Ravi,\n\nThanks, ask Priya Sharma about it.", email));
        assert!(!adds_specifics(suggested, "Hello Ravi,\n\nThanks, we will confirm soon.", email));
    }
}
