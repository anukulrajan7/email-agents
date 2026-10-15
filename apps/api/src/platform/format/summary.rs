//! Summaries in a human chat voice. A model's summary is checked and rewritten in code: every amount must be in
//! the email and is shown as ₹1,23,456.78; masked numbers become "card ending 4755"; dates become "12 Oct".

use std::sync::LazyLock;

use regex::Regex;

use super::money::{self, find_amounts, number_keys};

pub const CHECK_AMOUNT: &str = "Check the amount in the email.";
const MISSING_AMOUNT: &str = "an amount";

/// Style rules and four examples shared by the analyze and understanding prompts.
macro_rules! summary_style {
    () => {
        "summary: one or two short sentences written to the mailbox owner (\"you\"), the key fact first, in plain everyday words, like a helpful colleague texting. No internal codes or jargon. Write rupees as ₹ with Indian grouping (₹1,23,456.78) and never \"INR\"; a masked card or account as \"card ending 4755\", never \"XX4755\"; dates as \"12 Oct\". Copy every amount and date from the email exactly; never calculate or guess one. Examples of good summaries:\n\
  bank debit -> \"₹2,500 was debited from your account ending 1234 on 3 Nov. Your balance is ₹41,200.\"\n\
  credit card spend -> \"You spent ₹8,138.76 on your card ending 4755 at Amazon on 12 Oct.\"\n\
  invoice due -> \"Acme Supplies sent invoice 2231 for ₹1,18,000. It is due on 30 Nov.\"\n\
  salary credit -> \"Your salary of ₹85,000 was credited to your account ending 9876 on 1 Oct.\"\n"
    };
}
pub(crate) use summary_style;

const AMT: &str = r"-?(?:[₹$€£]|AED )[\d,]+(?:\.\d+)?";

static SPENT_FIRST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"(^|[.!?]\s+)({AMT}) (?:was |were |has been )?(spent|paid|received)\b")).expect("spent regex")
});
static PASSIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"({AMT}) (?:was |were |has been )?(debited|credited|refunded|withdrawn|deposited|transferred)\b")).expect("passive regex")
});
static YOUR_ACCOUNT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(on|from|to|with|using|in) (card|account) ending\b").expect("account regex"));
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]+").expect("spaces regex"));
static MERCHANT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:at|towards)\s+([A-Z][A-Za-z0-9&.'-]*(?:\s+[A-Z0-9][A-Za-z0-9&.'-]*){0,2})").expect("merchant regex")
});

fn structure(text: &str) -> String {
    let t = SPENT_FIRST.replace_all(text, "${1}You ${3} ${2}");
    let t = PASSIVE.replace_all(&t, "$1 was $2");
    YOUR_ACCOUNT.replace_all(&t, "$1 your $2 ending").into_owned()
}

fn tidy_up(text: &str, as_sentence: bool) -> String {
    let t = SPACES.replace_all(text.trim(), " ").replace(" ,", ",").replace(" .", ".");
    if !as_sentence {
        return t;
    }
    let mut chars = t.chars();
    let mut out: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => return t,
    };
    if out.ends_with(|c: char| c.is_alphanumeric()) {
        out.push('.');
    }
    out
}

fn finish(text: &str, known: Option<&str>, year: i32, as_sentence: bool) -> String {
    let (mut out, dropped) = match known {
        Some(source) => {
            let keys = number_keys(source);
            money::rewrite_amounts(text, &|k| keys.iter().any(|n| n == k), MISSING_AMOUNT)
        }
        None => money::rewrite_amounts(text, &|_| true, MISSING_AMOUNT),
    };
    out = money::rewrite_masked(&out, known);
    out = money::rewrite_dates(&out, year);
    out = tidy_up(&structure(&out), as_sentence);
    if dropped > 0 && !out.contains(CHECK_AMOUNT) {
        out = format!("{out} {CHECK_AMOUNT}");
    }
    out
}

/// A model's summary of an email, made safe and readable. `source` is everything the model saw (subject, body,
/// attachment text); an amount that is not in it is replaced and flagged. `received_ms` decides which year is "this year".
pub fn polish(summary: &str, source: &str, received_ms: i64) -> String {
    finish(summary, Some(source), money::year_of_ms(received_ms), true)
}

/// Same formatting for free text that has no single source email (assistant answers): nothing is dropped.
pub fn tidy(text: &str, now_ms: i64) -> String {
    finish(text, None, money::year_of_ms(now_ms), false)
}

fn merchant(text: &str) -> Option<String> {
    let name = MERCHANT.captures(text)?.get(1)?.as_str().trim_end_matches(['.', ',']).to_string();
    let shout = name.len() > 3 && name.chars().filter(|c| c.is_alphabetic()).all(char::is_uppercase);
    Some(if shout {
        name.split_whitespace()
            .map(|w| {
                let mut c = w.chars();
                c.next().map(|f| f.to_uppercase().chain(c.flat_map(char::to_lowercase)).collect::<String>()).unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        name
    })
}

fn first_sentences(text: &str, max_words: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().take(max_words).collect();
    let joined = words.join(" ");
    let mut ends = joined.match_indices(['.', '!', '?']).map(|(i, _)| i + 1).filter(|i| *i >= 20);
    match (ends.next(), ends.next()) {
        (_, Some(second)) => joined[..second].to_string(),
        (Some(first), None) => joined[..first].to_string(),
        _ => joined,
    }
}

/// A plain summary without a model: bank, card, invoice and salary mail get a sentence in the chat voice, anything
/// else the first sentences of the email.
pub fn rules_summary(subject: &str, body: &str, now_ms: i64) -> String {
    let year = money::year_of_ms(now_ms);
    let text = format!("{subject}\n{body}");
    let lower = text.to_lowercase();
    let amount = find_amounts(&text).into_iter().next().map(|a| a.render());
    let mask = money::find_masked(&text);
    let on = |t: &str| money::find_date(t, year).map(|d| format!(" on {d}")).unwrap_or_default();
    let at = merchant(body).map(|m| format!(" at {m}")).unwrap_or_default();
    let said = |words: &[&str]| words.iter().any(|w| lower.contains(w));

    let composed = amount.and_then(|amt| {
        let your = |fallback: &str| mask.clone().map_or_else(|| fallback.to_string(), |m| format!("your {m}"));
        if said(&["due"]) && said(&["invoice", "bill", "payment"]) {
            let after_due = lower.find("due").map_or("", |i| &text[i..]);
            let what = if said(&["invoice"]) { "invoice" } else if said(&["bill"]) { "bill" } else { "payment" };
            let when = Some(on(after_due)).filter(|d| !d.is_empty()).unwrap_or_else(|| on(&text));
            return Some(format!("Your {what} of {amt} is due{when}."));
        }
        if said(&["salary", "payroll"]) && said(&["credited", "deposited"]) {
            return Some(format!("Your salary of {amt} was credited to {}{}.", your("your account"), on(&text)));
        }
        if said(&["refund"]) {
            return Some(format!("You got a refund of {amt}{}.", on(&text)));
        }
        if said(&["credited", "deposited"]) {
            return Some(format!("{amt} was credited to {}{}.", your("your account"), on(&text)));
        }
        if said(&["spent", "purchase"]) || (said(&["debited"]) && said(&["card"])) {
            return Some(format!("You spent {amt} on {}{at}{}.", your("your card"), on(&text)));
        }
        if said(&["debited", "withdrawn"]) {
            return Some(format!("{amt} was debited from {}{at}{}.", your("your account"), on(&text)));
        }
        None
    });
    let body_text = if body.trim().is_empty() { subject } else { body };
    let plain = composed.unwrap_or_else(|| first_sentences(body_text, 40));
    let out = finish(&plain, None, year, true);
    if out.is_empty() { subject.trim().to_string() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OCT_2025: i64 = 1_760_270_000_000;

    #[test]
    fn robotic_card_summary_becomes_chat_style() {
        let source = "Rs.8138.76 spent on HDFC Bank Credit Card XX4755 at AMAZON on 12-10-2025";
        let out = polish("INR 8,138.76 spent on credit card XX4755 at Amazon on 12-10-2025.", source, OCT_2025);
        assert_eq!(out, "You spent ₹8,138.76 on your card ending 4755 at Amazon on 12 Oct.");
    }

    #[test]
    fn invented_amounts_are_dropped_and_flagged() {
        let out = polish("You spent ₹9,999 on your card ending 4755.", "Rs.8138.76 spent on Card XX4755", OCT_2025);
        assert_eq!(out, "You spent an amount on your card ending 4755. Check the amount in the email.");
        assert!(!out.contains("9,999"));
    }

    #[test]
    fn debits_and_credits_read_naturally() {
        let source = "INR 2,500.00 debited from A/c XX1234 on 03-11-2025";
        assert_eq!(
            polish("INR 2,500.00 debited from account XX1234 on 03-11-2025", source, OCT_2025),
            "₹2,500 was debited from your account ending 1234 on 3 Nov."
        );
    }

    #[test]
    fn a_digit_string_not_in_the_email_is_not_shown_as_a_card() {
        assert_eq!(polish("You spent ₹10 on your card ending 1111.", "₹10 spent on card XX2222", OCT_2025), "You spent ₹10 on your card.");
    }

    #[test]
    fn text_without_money_is_left_alone() {
        assert_eq!(polish("Priya asks for leave next Friday.", "Can I take leave?", OCT_2025), "Priya asks for leave next Friday.");
    }

    #[test]
    fn tidy_formats_without_dropping() {
        assert_eq!(tidy("You paid INR 12,00,000 on 12/10/2024.", OCT_2025), "You paid ₹12,00,000 on 12 Oct 2024.");
    }

    #[test]
    fn rules_summary_in_chat_voice() {
        let card = rules_summary(
            "Transaction alert",
            "Rs.8138.76 spent on your HDFC Bank Credit Card ending 4755 at AMAZON on 12-10-2025. Not you? Call us.",
            OCT_2025,
        );
        assert_eq!(card, "You spent ₹8,138.76 on your card ending 4755 at Amazon on 12 Oct.");
        let salary = rules_summary("Salary credit", "INR 85,000.00 credited to A/c XX9876 on 01-10-2025 towards salary.", OCT_2025);
        assert_eq!(salary, "Your salary of ₹85,000 was credited to your account ending 9876 on 1 Oct.");
        let invoice = rules_summary("Invoice 2231", "Please pay Rs. 1,18,000 against invoice 2231 by 30 November 2025. Payment is due.", OCT_2025);
        assert_eq!(invoice, "Your invoice of ₹1,18,000 is due on 30 Nov.");
    }

    #[test]
    fn rules_summary_falls_back_to_the_first_sentences() {
        let out = rules_summary("Leave", "Hi team, can I take leave next Friday? I will finish my handover first. Thanks a lot for your help.", OCT_2025);
        assert_eq!(out, "Hi team, can I take leave next Friday? I will finish my handover first.");
    }
}
