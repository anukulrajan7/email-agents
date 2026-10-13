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

/// `staffing`: the workspace has the staffing pack, so its client/referral labels count as premium.
pub fn draft_quality(
    priority: Priority,
    label: &str,
    insight: Option<&Insight>,
    requested: Option<DraftQuality>,
    staffing: bool,
) -> DraftQuality {
    let complex = insight.is_some_and(|i| i.asks.len() >= PREMIUM_ASKS || !i.missing_info.is_empty());
    let premium_label = staffing && PREMIUM_LABELS.contains(&label);
    if requested == Some(DraftQuality::Premium) || priority == Priority::High || premium_label || complex {
        DraftQuality::Premium
    } else {
        DraftQuality::Standard
    }
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
        assert_eq!(draft_quality(Priority::Normal, "leave", Some(&insight(2, 0)), None, true), DraftQuality::Standard);
        assert_eq!(draft_quality(Priority::Low, "timesheet", None, None, true), DraftQuality::Standard);
    }

    #[test]
    fn high_priority_is_premium() {
        assert_eq!(draft_quality(Priority::High, "leave", None, None, true), DraftQuality::Premium);
    }

    #[test]
    fn client_and_referral_labels_are_premium() {
        assert_eq!(draft_quality(Priority::Normal, "client_requirement", None, None, true), DraftQuality::Premium);
        assert_eq!(draft_quality(Priority::Low, "candidate_referral", None, None, true), DraftQuality::Premium);
    }

    #[test]
    fn staffing_labels_are_standard_without_the_staffing_pack() {
        assert_eq!(draft_quality(Priority::Normal, "client_requirement", None, None, false), DraftQuality::Standard);
        assert_eq!(draft_quality(Priority::High, "client_requirement", None, None, false), DraftQuality::Premium);
        assert_eq!(draft_quality(Priority::Normal, "question", Some(&insight(3, 0)), None, false), DraftQuality::Premium);
    }

    #[test]
    fn many_asks_or_missing_info_is_premium() {
        assert_eq!(draft_quality(Priority::Normal, "leave", Some(&insight(3, 0)), None, true), DraftQuality::Premium);
        assert_eq!(draft_quality(Priority::Normal, "leave", Some(&insight(1, 1)), None, true), DraftQuality::Premium);
    }

    #[test]
    fn explicit_request_is_premium() {
        assert_eq!(draft_quality(Priority::Low, "timesheet", None, Some(DraftQuality::Premium), true), DraftQuality::Premium);
        assert_eq!(draft_quality(Priority::Low, "timesheet", None, Some(DraftQuality::Standard), true), DraftQuality::Standard);
    }
}
