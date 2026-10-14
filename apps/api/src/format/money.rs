//! Money, masked numbers and dates as people say them: ₹1,23,456.78, "card ending 4755", "12 Oct".

use std::sync::LazyLock;

use regex::{Captures, Regex};

const NUM: &str = r"\d+(?:,\d{2,3})*(?:\.\d+)?";

/// One amount found in text. Digits are kept as text so no rounding can ever happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Amount {
    /// ISO-style code: INR, USD, EUR, GBP, AED.
    pub currency: &'static str,
    pub negative: bool,
    /// Digits before the point, no separators, no leading zeros ("0" when empty).
    pub whole: String,
    /// Digits after the point with trailing zeros removed (empty = a whole number).
    pub fraction: String,
}

impl Amount {
    pub fn parse(currency: &'static str, negative: bool, number: &str) -> Option<Amount> {
        let cleaned = number.replace(',', "");
        let (whole, fraction) = cleaned.split_once('.').unwrap_or((cleaned.as_str(), ""));
        if whole.is_empty() || !whole.chars().all(|c| c.is_ascii_digit()) || !fraction.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let whole = whole.trim_start_matches('0');
        Some(Amount {
            currency,
            negative,
            whole: if whole.is_empty() { "0".into() } else { whole.into() },
            fraction: fraction.trim_end_matches('0').to_string(),
        })
    }

    /// The number alone, as the key used to check an amount against the email.
    pub fn key(&self) -> String {
        number_key(&self.whole, &self.fraction)
    }

    /// Canonical text: ₹1,23,456.78, $1,234.50, -₹500. Two decimals whenever there are any.
    pub fn render(&self) -> String {
        let grouped = if self.currency == "INR" { group_indian(&self.whole) } else { group_standard(&self.whole) };
        let fraction = match self.fraction.len() {
            0 => String::new(),
            1 => format!(".{}0", self.fraction),
            _ => format!(".{}", self.fraction),
        };
        let sign = if self.negative { "-" } else { "" };
        let symbol = match self.currency {
            "INR" => "₹",
            "USD" => "$",
            "EUR" => "€",
            "GBP" => "£",
            "AED" => "AED ",
            _ => "",
        };
        format!("{sign}{symbol}{grouped}{fraction}")
    }
}

fn number_key(whole: &str, fraction: &str) -> String {
    let whole = whole.trim_start_matches('0');
    let whole = if whole.is_empty() { "0" } else { whole };
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() { whole.to_string() } else { format!("{whole}.{fraction}") }
}

/// `1234567` becomes `12,34,567`.
pub fn group_indian(whole: &str) -> String {
    if whole.len() <= 3 {
        return whole.to_string();
    }
    let (head, tail) = whole.split_at(whole.len() - 3);
    let mut groups: Vec<&str> = Vec::new();
    let mut end = head.len();
    while end > 0 {
        let start = end.saturating_sub(2);
        groups.push(&head[start..end]);
        end = start;
    }
    groups.reverse();
    format!("{},{tail}", groups.join(","))
}

/// `1234567` becomes `1,234,567`.
pub fn group_standard(whole: &str) -> String {
    let mut out = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn currency_of(marker: &str) -> Option<&'static str> {
    match marker.trim_end_matches('.').to_lowercase().as_str() {
        "inr" | "rs" | "₹" | "rupee" | "rupees" => Some("INR"),
        "usd" | "us$" | "$" => Some("USD"),
        "eur" | "€" => Some("EUR"),
        "gbp" | "£" => Some("GBP"),
        "aed" => Some("AED"),
        _ => None,
    }
}

static AMOUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)(?P<neg0>-)?(?:(?P<pre>\b(?:INR|Rs\.?|USD|US\$|EUR|GBP|AED)|₹|\$|€|£)\s?(?P<neg1>-)?\s?(?P<n1>{NUM})|(?P<n2>{NUM})\s?(?P<post>\b(?:INR|rupees?|USD|EUR|GBP|AED)\b))"
    ))
    .expect("amount regex")
});

fn amount_of(c: &Captures) -> Option<Amount> {
    let (marker, number) = match (c.name("pre"), c.name("n1"), c.name("n2"), c.name("post")) {
        (Some(p), Some(n), _, _) => (p.as_str(), n.as_str()),
        (_, _, Some(n), Some(p)) => (p.as_str(), n.as_str()),
        _ => return None,
    };
    Amount::parse(currency_of(marker)?, c.name("neg0").is_some() || c.name("neg1").is_some(), number)
}

/// Every amount with a currency marker in the text, in order.
pub fn find_amounts(text: &str) -> Vec<Amount> {
    AMOUNT.captures_iter(text).filter_map(|c| amount_of(&c)).collect()
}

static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(NUM).expect("number regex"));

/// Every number in the text as an amount key (no separators, no padding zeros): what an amount may be checked against.
pub fn number_keys(text: &str) -> Vec<String> {
    NUMBER
        .find_iter(text)
        .filter_map(|m| Amount::parse("INR", false, m.as_str()))
        .map(|a| a.key())
        .collect()
}

/// Rewrites every amount in the text to the canonical form. `check` is asked about each amount's key: when it
/// returns false the amount is replaced by `placeholder` and counted as dropped.
pub fn rewrite_amounts(text: &str, check: &dyn Fn(&str) -> bool, placeholder: &str) -> (String, usize) {
    let mut dropped = 0;
    let out = AMOUNT.replace_all(text, |c: &Captures| match amount_of(c) {
        Some(a) if check(&a.key()) => a.render(),
        Some(_) => {
            dropped += 1;
            placeholder.to_string()
        }
        None => c[0].to_string(),
    });
    (out.into_owned(), dropped)
}

// ---------------------------------------------------------------- masked numbers

static MASKED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:\b(?P<kind>credit card|debit card|card|a/c|acct|account)(?:\s+(?:no\.?|number|num))?\s*[:\-]?\s*)?(?:(?:[x*•]{2,}[\s-]*)+(?P<d1>\d{4})\b|\bending\s+(?:in\s+|with\s+)?(?P<d2>\d{4})\b)",
    )
    .expect("masked regex")
});

/// "credit card XX4755", "A/c ****4755", "card ending in 4755" all become "card ending 4755" (or "account ending ...").
/// `known`, when given, is the email text: a last-four that is not in it is dropped, leaving just "card" / "account".
pub fn rewrite_masked(text: &str, known: Option<&str>) -> String {
    MASKED
        .replace_all(text, |c: &Captures| {
            let kind = c.name("kind").map(|k| k.as_str().to_lowercase());
            let word = match kind.as_deref() {
                Some(k) if k.starts_with("a/c") || k.starts_with("acct") || k.starts_with("account") => "account",
                _ => "card",
            };
            if kind.is_none() && c.name("d1").is_none() {
                return c[0].to_string();
            }
            let digits = c.name("d1").or_else(|| c.name("d2")).map_or("", |d| d.as_str());
            if known.is_some_and(|k| !k.contains(digits)) {
                return word.to_string();
            }
            format!("{word} ending {digits}")
        })
        .into_owned()
}

/// The first masked number in the text as "card ending 4755" / "account ending 4755".
pub fn find_masked(text: &str) -> Option<String> {
    let found = MASKED.find(text)?;
    Some(rewrite_masked(found.as_str(), None)).filter(|m| m.contains("ending"))
}

// ---------------------------------------------------------------- dates

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

fn month_index(name: &str) -> Option<usize> {
    let lower = name.to_lowercase();
    MONTHS.iter().position(|m| lower.starts_with(&m.to_lowercase()))
}

/// "12 Oct", and the year only when it is not `current_year`.
pub fn short_date(day: u32, month: usize, year: Option<i32>, current_year: i32) -> Option<String> {
    if !(1..=31).contains(&day) || month >= 12 {
        return None;
    }
    Some(match year.filter(|y| *y != current_year) {
        Some(y) => format!("{day} {} {y}", MONTHS[month]),
        None => format!("{day} {}", MONTHS[month]),
    })
}

const MONTH_NAMES: &str = r"(?:Jan(?:uary)?|Feb(?:ruary)?|Mar(?:ch)?|Apr(?:il)?|May|June?|July?|Aug(?:ust)?|Sep(?:t(?:ember)?)?|Oct(?:ober)?|Nov(?:ember)?|Dec(?:ember)?)";

static DATE_NUMERIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(\d{1,2})[-/.](\d{1,2})[-/.](\d{4}|\d{2})\b").expect("date regex"));
static DATE_ISO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(20\d{2})-(\d{2})-(\d{2})\b").expect("date regex"));
static DATE_DAY_FIRST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"\b(\d{{1,2}})(?:st|nd|rd|th)?[\s-]+({MONTH_NAMES})\b(?:,?[\s-]+(20\d{{2}})\b)?")).expect("date regex")
});
static DATE_MONTH_FIRST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"\b({MONTH_NAMES})\b\.?\s+(\d{{1,2}})(?:st|nd|rd|th)?\b(?:,?\s+(20\d{{2}})\b)?")).expect("date regex")
});

fn year_of(raw: &str) -> Option<i32> {
    let y: i32 = raw.parse().ok()?;
    Some(if raw.len() == 2 { 2000 + y } else { y })
}

/// Rewrites dates (12-10-2025, 2025-10-12, 12 October 2025, Oct 12) to "12 Oct", adding the year only when it
/// differs from `current_year`.
pub fn rewrite_dates(text: &str, current_year: i32) -> String {
    let t = DATE_ISO.replace_all(text, |c: &Captures| {
        let (y, m, d) = (year_of(&c[1]), c[2].parse::<usize>().ok(), c[3].parse::<u32>().ok());
        m.zip(d)
            .and_then(|(m, d)| short_date(d, m.checked_sub(1)?, y, current_year))
            .unwrap_or_else(|| c[0].to_string())
    });
    let t = DATE_NUMERIC.replace_all(&t, |c: &Captures| {
        let (d, m) = (c[1].parse::<u32>().ok(), c[2].parse::<usize>().ok());
        d.zip(m)
            .filter(|(_, m)| (1..=12).contains(m))
            .and_then(|(d, m)| short_date(d, m - 1, year_of(&c[3]), current_year))
            .unwrap_or_else(|| c[0].to_string())
    });
    let t = DATE_DAY_FIRST.replace_all(&t, |c: &Captures| {
        let (d, m) = (c[1].parse::<u32>().ok(), month_index(&c[2]));
        d.zip(m)
            .and_then(|(d, m)| short_date(d, m, c.get(3).and_then(|y| year_of(y.as_str())), current_year))
            .unwrap_or_else(|| c[0].to_string())
    });
    DATE_MONTH_FIRST
        .replace_all(&t, |c: &Captures| {
            let (m, d) = (month_index(&c[1]), c[2].parse::<u32>().ok());
            m.zip(d)
                .and_then(|(m, d)| short_date(d, m, c.get(3).and_then(|y| year_of(y.as_str())), current_year))
                .unwrap_or_else(|| c[0].to_string())
        })
        .into_owned()
}

/// The first date in the text, in the short form.
pub fn find_date(text: &str, current_year: i32) -> Option<String> {
    [&*DATE_ISO, &*DATE_NUMERIC, &*DATE_DAY_FIRST, &*DATE_MONTH_FIRST]
        .into_iter()
        .filter_map(|re| re.find(text))
        .min_by_key(|m| m.start())
        .map(|m| rewrite_dates(m.as_str(), current_year))
        .filter(|d| d.chars().any(|c| c.is_alphabetic()))
}

/// The calendar year of a unix-millisecond instant (UTC).
pub fn year_of_ms(ms: i64) -> i32 {
    time::OffsetDateTime::from_unix_timestamp(ms.div_euclid(1000)).map(|d| d.year()).unwrap_or(2026)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(currency: &'static str, number: &str) -> String {
        Amount::parse(currency, false, number).unwrap().render()
    }

    #[test]
    fn indian_grouping() {
        assert_eq!(render("INR", "123456.78"), "₹1,23,456.78");
        assert_eq!(render("INR", "8138.76"), "₹8,138.76");
        assert_eq!(render("INR", "999"), "₹999");
        assert_eq!(render("INR", "1000"), "₹1,000");
        assert_eq!(render("INR", "12345678"), "₹1,23,45,678");
        assert_eq!(render("INR", "1,23,456"), "₹1,23,456");
        assert_eq!(group_indian("100000"), "1,00,000");
    }

    #[test]
    fn paise_are_kept_to_two_digits_and_zero_paise_dropped() {
        assert_eq!(render("INR", "500.5"), "₹500.50");
        assert_eq!(render("INR", "500.00"), "₹500");
        assert_eq!(render("INR", "0.75"), "₹0.75");
        assert_eq!(render("INR", "007.05"), "₹7.05");
    }

    #[test]
    fn other_currencies_use_standard_grouping_and_their_symbol() {
        assert_eq!(render("USD", "1234.5"), "$1,234.50");
        assert_eq!(render("USD", "1234567"), "$1,234,567");
        assert_eq!(render("EUR", "1234.5"), "€1,234.50");
        assert_eq!(render("GBP", "99"), "£99");
        assert_eq!(render("AED", "1500"), "AED 1,500");
    }

    #[test]
    fn negative_amounts_and_refunds() {
        assert_eq!(Amount::parse("INR", true, "1200").unwrap().render(), "-₹1,200");
        let found = find_amounts("Reversal of -Rs. 1,200.00 and refund of Rs 450");
        assert_eq!(found.iter().map(Amount::render).collect::<Vec<_>>(), ["-₹1,200", "₹450"]);
        assert_eq!(find_amounts("charged ₹-75.5")[0].render(), "-₹75.50");
    }

    #[test]
    fn finds_amounts_in_every_common_spelling() {
        let text = "INR 8,138.76, Rs.1,00,000/-, ₹ 250, 3,500 INR, USD 1,234.50, $99, 12 rupees";
        let rendered: Vec<String> = find_amounts(text).iter().map(Amount::render).collect();
        assert_eq!(rendered, ["₹8,138.76", "₹1,00,000", "₹250", "₹3,500", "$1,234.50", "$99", "₹12"]);
        assert!(find_amounts("Order 12345 shipped, version 2.0").is_empty());
        assert!(find_amounts("Hers 500 items").is_empty());
    }

    #[test]
    fn keys_ignore_formatting() {
        let keys = number_keys("Rs.8138.76 and 8,138.76 and 5,000.00");
        assert_eq!(keys, ["8138.76", "8138.76", "5000"]);
    }

    #[test]
    fn rewrites_amounts_and_drops_unknown_ones() {
        let known = number_keys("Rs.8138.76 spent");
        let (out, dropped) = rewrite_amounts("INR 8,138.76 and INR 9,999 charged", &|k| known.iter().any(|n| n == k), "an amount");
        assert_eq!((out.as_str(), dropped), ("₹8,138.76 and an amount charged", 1));
    }

    #[test]
    fn masked_numbers() {
        assert_eq!(rewrite_masked("on credit card XX4755 at Amazon", None), "on card ending 4755 at Amazon");
        assert_eq!(rewrite_masked("A/c XXXX1234 debited", None), "account ending 1234 debited");
        assert_eq!(rewrite_masked("card no. XXXX-XXXX-XXXX-4755", None), "card ending 4755");
        assert_eq!(rewrite_masked("account ****9876", None), "account ending 9876");
        assert_eq!(rewrite_masked("your card ending in 4755", None), "your card ending 4755");
        assert_eq!(rewrite_masked("Debit Card ending 4755", None), "card ending 4755");
        assert_eq!(rewrite_masked("card XX4755", Some("Card XX4755 used")), "card ending 4755");
        assert_eq!(rewrite_masked("card XX9999", Some("Card XX4755 used")), "card");
    }

    #[test]
    fn dates_drop_the_current_year() {
        assert_eq!(rewrite_dates("on 12-10-2025.", 2025), "on 12 Oct.");
        assert_eq!(rewrite_dates("on 12/10/25", 2025), "on 12 Oct");
        assert_eq!(rewrite_dates("on 12-10-2024", 2025), "on 12 Oct 2024");
        assert_eq!(rewrite_dates("due 2025-11-03", 2025), "due 3 Nov");
        assert_eq!(rewrite_dates("due 2024-11-03", 2025), "due 3 Nov 2024");
        assert_eq!(rewrite_dates("on 12 October 2025", 2025), "on 12 Oct");
        assert_eq!(rewrite_dates("on 12-Oct-2025", 2025), "on 12 Oct");
        assert_eq!(rewrite_dates("by October 12, 2025", 2025), "by 12 Oct");
        assert_eq!(rewrite_dates("by 5th November 2023", 2025), "by 5 Nov 2023");
        assert_eq!(rewrite_dates("on 12 Oct", 2025), "on 12 Oct");
        assert_eq!(rewrite_dates("version 1.2.3 of 99.99.99", 2025), "version 1.2.3 of 99.99.99");
    }

    #[test]
    fn year_from_millis() {
        assert_eq!(year_of_ms(1_760_000_000_000), 2025);
    }
}
