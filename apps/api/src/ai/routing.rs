//! Picks the drafting tier. Deterministic and cheap: most replies are routine, so the
//! standard tier writes them; the premium tier is kept for mail where a mistake costs more.

use super::{DraftQuality, Insight};
use crate::domain::Priority;

/// Staffing labels whose emails always get the premium tier (only in staffing workspaces).
const PREMIUM_LABELS: &[&str] = &["client_requirement", "candidate_referral"];
/// This many asks (or more) in one email are easy for a small model to drop.
const PREMIUM_ASKS: usize = 3;

/// Staffing pack business context used when a workspace has not written its own reply
/// instructions. Other packs use `packs::NEUTRAL_REPLY_STYLE`; pick with `packs::reply_style`.
pub const DEFAULT_REPLY_STYLE: &str = "We are the company's HR and staffing team.\n\
- Client job requirement (a company needs people): thank them, confirm we will share matching candidate profiles, and ask only for missing essentials (location, budget, start date, number of positions) using [NEEDS: ...].\n\
- Candidate application or referral (a person wants a job): thank them, say we will match their profile with open roles, and ask for their CV if none is attached.\n\
- Leave, timesheet or other staff requests: acknowledge and say who will follow up; never approve or reject.\n\
- Never reject anyone and never say we are not hiring.\n\
Keep replies 60 to 120 words, in short paragraphs, signed off.";

/// True when `sender` is one of the owner's important people: an exact address, or any address at a listed domain.
pub fn is_vip(vips: &[String], sender: &str) -> bool {
    let sender = sender.trim().to_ascii_lowercase();
    let domain = sender.rsplit_once('@').map(|(_, d)| d).unwrap_or_default();
    vips.iter().map(|v| v.trim().trim_start_matches('@').to_ascii_lowercase()).any(|v| !v.is_empty() && (v == sender || v == domain))
}

/// Premium only where a mistake costs more: high priority, a VIP sender, three or more asks, or (staffing
/// workspaces) client and referral labels. Missing information alone does not qualify: about half of real
/// mail has some, and the draft already marks it with `[NEEDS: …]`.
/// `staffing`: the workspace has the staffing pack, so its client/referral labels count as premium.
pub fn draft_quality(
    priority: Priority,
    label: &str,
    insight: Option<&Insight>,
    requested: Option<DraftQuality>,
    staffing: bool,
    vip: bool,
) -> DraftQuality {
    let complex = insight.is_some_and(|i| i.asks.len() >= PREMIUM_ASKS);
    let premium_label = staffing && PREMIUM_LABELS.contains(&label);
    if requested == Some(DraftQuality::Premium) || priority == Priority::High || vip || premium_label || complex {
        DraftQuality::Premium
    } else {
        DraftQuality::Standard
    }
}

/// Which model reads a timesheet. The cheap pass goes first; the stronger one is asked only when the cheap pass
/// failed its checks (totals that do not add up, no period) or the sheet is too long for the cheap one to hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetTier {
    Cheap,
    Strong,
}

/// A sheet this long (characters) or this many rows goes straight to the strong tier.
pub const SHEET_LARGE_CHARS: usize = 14_000;
pub const SHEET_LARGE_ROWS: usize = 150;

pub fn sheet_tier(chars: usize, rows: usize, cheap_failed: bool) -> SheetTier {
    if cheap_failed || chars > SHEET_LARGE_CHARS || rows > SHEET_LARGE_ROWS { SheetTier::Strong } else { SheetTier::Cheap }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insight(asks: usize, missing: usize) -> Insight {
        Insight {
            summary: "s".into(),
            asks: (0..asks).map(|i| format!("ask {i}")).collect(),
            deadline: None,
            needs_reply: true,
            missing_info: (0..missing).map(|i| format!("gap {i}")).collect(),
        }
    }

    #[test]
    fn routine_mail_is_standard() {
        assert_eq!(draft_quality(Priority::Normal, "leave", Some(&insight(2, 0)), None, true, false), DraftQuality::Standard);
        assert_eq!(draft_quality(Priority::Low, "timesheet", None, None, true, false), DraftQuality::Standard);
    }

    #[test]
    fn high_priority_is_premium() {
        assert_eq!(draft_quality(Priority::High, "leave", None, None, true, false), DraftQuality::Premium);
    }

    #[test]
    fn client_and_referral_labels_are_premium() {
        assert_eq!(draft_quality(Priority::Normal, "client_requirement", None, None, true, false), DraftQuality::Premium);
        assert_eq!(draft_quality(Priority::Low, "candidate_referral", None, None, true, false), DraftQuality::Premium);
    }

    #[test]
    fn staffing_labels_are_standard_without_the_staffing_pack() {
        assert_eq!(draft_quality(Priority::Normal, "client_requirement", None, None, false, false), DraftQuality::Standard);
        assert_eq!(draft_quality(Priority::High, "client_requirement", None, None, false, false), DraftQuality::Premium);
        assert_eq!(draft_quality(Priority::Normal, "question", Some(&insight(3, 0)), None, false, false), DraftQuality::Premium);
    }

    #[test]
    fn three_asks_are_premium_but_missing_info_alone_is_not() {
        assert_eq!(draft_quality(Priority::Normal, "leave", Some(&insight(3, 0)), None, true, false), DraftQuality::Premium);
        assert_eq!(draft_quality(Priority::Normal, "leave", Some(&insight(2, 0)), None, true, false), DraftQuality::Standard);
        assert_eq!(draft_quality(Priority::Normal, "leave", Some(&insight(1, 4)), None, true, false), DraftQuality::Standard);
        assert_eq!(draft_quality(Priority::Normal, "leave", Some(&insight(0, 1)), None, false, false), DraftQuality::Standard);
    }

    #[test]
    fn a_vip_sender_is_premium() {
        assert_eq!(draft_quality(Priority::Normal, "leave", None, None, false, true), DraftQuality::Premium);
        let vips = vec!["ceo@client.com".to_string(), "bigco.com".to_string()];
        assert!(is_vip(&vips, "CEO@Client.com"));
        assert!(is_vip(&vips, "anyone@bigco.com"));
        assert!(!is_vip(&vips, "intern@client.com"));
        assert!(!is_vip(&[], "ceo@client.com"));
        assert!(!is_vip(&["".to_string()], "ceo@client.com"));
    }

    #[test]
    fn explicit_request_is_premium() {
        assert_eq!(draft_quality(Priority::Low, "timesheet", None, Some(DraftQuality::Premium), true, false), DraftQuality::Premium);
        assert_eq!(draft_quality(Priority::Low, "timesheet", None, Some(DraftQuality::Standard), true, false), DraftQuality::Standard);
    }

    #[test]
    fn timesheets_go_to_the_strong_model_only_when_needed() {
        assert_eq!(sheet_tier(2_000, 30, false), SheetTier::Cheap);
        assert_eq!(sheet_tier(2_000, 30, true), SheetTier::Strong);
        assert_eq!(sheet_tier(SHEET_LARGE_CHARS + 1, 30, false), SheetTier::Strong);
        assert_eq!(sheet_tier(2_000, SHEET_LARGE_ROWS + 1, false), SheetTier::Strong);
    }
}
