//! Packs: use-case bundles a workspace turns on (labels, automation presets, reply style and
//! features). `general` is the default; HR/recruiting is the optional `staffing` pack.
//! Disabling a pack never deletes data: it only gates the pack's features.

use serde_json::{Value, json};
use sqlx::{SqliteConnection, SqliteExecutor, SqlitePool};

use crate::{
    ai::routing::DEFAULT_REPLY_STYLE,
    db::{new_id, now_ms},
    domain::{GENERAL_LABELS, HR_LABELS, LabelSeed, OTHER_LABEL, OTHER_SEED, WORKFLOW_TEMPLATES},
};

pub const GENERAL: &str = "general";
pub const STAFFING: &str = "staffing";

/// Structured CV / requirement extraction and candidate matching.
pub const FEATURE_MATCHING: &str = "matching";
/// Requirement cases started automatically by the agent.
pub const FEATURE_REQUIREMENT_AGENT: &str = "requirement_agent";
/// The built-in HR workflows (timesheet, leave, referral reviews).
pub const FEATURE_HR_WORKFLOWS: &str = "hr_workflows";

/// Reply style for every pack except staffing.
pub const NEUTRAL_REPLY_STYLE: &str = "Reply as the mailbox owner: acknowledge the email, answer only from known facts, \
and mark anything you don't know with [NEEDS: ...].\n\
Keep replies 50 to 140 words, in short paragraphs, signed off.";

/// Sign-off when a staffing workspace has none of its own (today's behaviour).
const STAFFING_SIGN_OFF: &str = "HR Operations";
const FALLBACK_SIGN_OFF: &str = "The team";

const GENERIC_PRESETS: &[&str] = &["reply_to_questions", "chase_important"];

pub struct Pack {
    pub key: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub labels: &'static [LabelSeed],
    /// Automation presets offered in this pack's workspaces.
    pub presets: &'static [&'static str],
    pub reply_style: &'static str,
    pub features: &'static [&'static str],
}

const fn general_seed(description: &'static str) -> LabelSeed {
    LabelSeed { key: "general", name: "General", description, color: "ink" }
}

pub const PACKS: &[Pack] = &[
    Pack {
        key: GENERAL,
        name: "Business inbox",
        description: "Things to act on, questions, customers, leads, billing and scheduling.",
        labels: GENERAL_LABELS,
        presets: GENERIC_PRESETS,
        reply_style: NEUTRAL_REPLY_STYLE,
        features: &[],
    },
    Pack {
        key: STAFFING,
        name: "HR & recruiting",
        description: "Timesheets, leave, referrals, client requirements, payroll and candidate matching.",
        labels: HR_LABELS,
        presets: &["reply_to_questions", "acknowledge_timesheets", "chase_important", "auto_ack_referrals"],
        reply_style: DEFAULT_REPLY_STYLE,
        features: &[FEATURE_MATCHING, FEATURE_REQUIREMENT_AGENT, FEATURE_HR_WORKFLOWS],
    },
    Pack {
        key: "support",
        name: "Customer support",
        description: "Customer questions, bug reports, complaints and refund requests.",
        labels: &[
            LabelSeed { key: "question", name: "Question", description: "A customer asks how something works or asks for information, with no problem to fix.", color: "slate" },
            LabelSeed { key: "bug_report", name: "Bug report", description: "A customer reports that something in our product or service is broken, failing or showing an error.", color: "plum" },
            LabelSeed { key: "complaint", name: "Complaint", description: "A customer is unhappy with our service, a delay or a person, or escalates an earlier issue.", color: "postmark" },
            LabelSeed { key: "refund_request", name: "Refund request", description: "A customer asks for a refund, a return, a cancellation with money back or a credit.", color: "teal" },
            general_seed("A legitimate customer email that fits no specific label above."),
            OTHER_SEED,
        ],
        presets: GENERIC_PRESETS,
        reply_style: NEUTRAL_REPLY_STYLE,
        features: &[],
    },
    Pack {
        key: "sales",
        name: "Sales",
        description: "New leads, follow-ups on proposals and meeting scheduling.",
        labels: &[
            LabelSeed { key: "sales_lead", name: "Sales lead", description: "A potential customer asks about pricing, a demo, a quote or buying from us for the first time.", color: "sage" },
            LabelSeed { key: "follow_up", name: "Follow-up", description: "A prospect or customer replies about an ongoing deal, proposal, quote or contract.", color: "postmark" },
            LabelSeed { key: "scheduling", name: "Scheduling", description: "Meeting or demo requests, calendar invites, reschedules or availability questions.", color: "ochre" },
            general_seed("A legitimate business email that fits no specific label above."),
            OTHER_SEED,
        ],
        presets: GENERIC_PRESETS,
        reply_style: NEUTRAL_REPLY_STYLE,
        features: &[],
    },
    Pack {
        key: "finance",
        name: "Finance",
        description: "Vendor invoices, payments and collections, expenses and reimbursements.",
        labels: &[
            LabelSeed { key: "invoice", name: "Invoice", description: "A vendor or supplier sends an invoice, a bill or a question about one we received.", color: "teal" },
            LabelSeed { key: "payment", name: "Payment", description: "A payment made, due, overdue or failed: remittance advice, reminders, collections or payment confirmations.", color: "postmark" },
            LabelSeed { key: "expense", name: "Expense", description: "An expense claim, receipt for reimbursement or a question about spending approval.", color: "ochre" },
            general_seed("A legitimate finance email that fits no specific label above."),
            OTHER_SEED,
        ],
        presets: GENERIC_PRESETS,
        reply_style: NEUTRAL_REPLY_STYLE,
        features: &[],
    },
    Pack {
        key: "personal",
        name: "Personal",
        description: "One person's inbox: what matters, what needs a reply, and the noise.",
        labels: &[
            LabelSeed { key: "important", name: "Important", description: "From a person I know or work with, about something that matters or has a deadline.", color: "postmark" },
            LabelSeed { key: "needs_reply", name: "Needs reply", description: "A real person asks me something or is waiting for my answer.", color: "slate" },
            LabelSeed { key: "newsletters", name: "Newsletters", description: "Newsletters, digests, marketing and announcements I subscribed to.", color: "ink" },
            LabelSeed { key: "receipts", name: "Receipts", description: "Order confirmations, receipts, bookings, shipping updates and bills.", color: "teal" },
            OTHER_SEED,
        ],
        presets: GENERIC_PRESETS,
        reply_style: NEUTRAL_REPLY_STYLE,
        features: &[],
    },
];

pub fn pack(key: &str) -> Option<&'static Pack> {
    PACKS.iter().find(|p| p.key == key)
}

/// Legacy `template` names from older clients.
pub fn from_template(template: &str) -> Option<&'static str> {
    match template {
        "hr" => Some(STAFFING),
        "general" | "blank" => Some(GENERAL),
        _ => None,
    }
}

/// The catalogue as the API shows it.
pub fn catalogue_json(p: &Pack) -> Value {
    json!({
        "key": p.key, "name": p.name, "description": p.description,
        "labels": p.labels.iter().map(|l| json!({"key": l.key, "name": l.name, "description": l.description})).collect::<Vec<_>>(),
        "presets": p.presets,
        "features": p.features,
    })
}

/// Enabled pack keys in catalogue order. A workspace with no rows is a plain business inbox.
pub async fn enabled_keys<'e>(db: impl SqliteExecutor<'e>, workspace_id: &str) -> sqlx::Result<Vec<&'static str>> {
    let rows: Vec<String> = sqlx::query_scalar("SELECT pack_key FROM workspace_packs WHERE workspace_id = ?1")
        .bind(workspace_id)
        .fetch_all(db)
        .await?;
    let keys: Vec<&'static str> = PACKS.iter().map(|p| p.key).filter(|k| rows.iter().any(|r| r == k)).collect();
    Ok(if keys.is_empty() { vec![GENERAL] } else { keys })
}

pub async fn enabled<'e>(db: impl SqliteExecutor<'e>, workspace_id: &str, key: &str) -> sqlx::Result<bool> {
    Ok(enabled_keys(db, workspace_id).await?.contains(&key))
}

pub async fn has_feature<'e>(db: impl SqliteExecutor<'e>, workspace_id: &str, feature: &str) -> sqlx::Result<bool> {
    Ok(enabled_keys(db, workspace_id).await?.iter().filter_map(|k| pack(k)).any(|p| p.features.contains(&feature)))
}

/// Default reply style: staffing keeps today's HR text verbatim, every other pack is neutral.
pub async fn reply_style(db: &SqlitePool, workspace_id: &str) -> sqlx::Result<&'static str> {
    Ok(if enabled(db, workspace_id, STAFFING).await? { DEFAULT_REPLY_STYLE } else { NEUTRAL_REPLY_STYLE })
}

/// Sign-off used when the workspace has not set one: "HR Operations" for staffing, else the workspace name.
pub async fn default_sign_off(db: &SqlitePool, workspace_id: &str) -> sqlx::Result<String> {
    if enabled(db, workspace_id, STAFFING).await? {
        return Ok(STAFFING_SIGN_OFF.into());
    }
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM workspaces WHERE id = ?1")
        .bind(workspace_id)
        .fetch_optional(db)
        .await?;
    let clean: String = name.unwrap_or_default().chars().filter(|c| !c.is_control() && *c != '"').collect();
    Ok(match clean.trim() {
        "" => FALLBACK_SIGN_OFF.into(),
        n => n.to_string(),
    })
}

/// Presets offered for these packs, in catalogue order.
pub fn presets_for(keys: &[&str]) -> Vec<&'static str> {
    crate::automations::PRESET_KEYS
        .iter()
        .copied()
        .filter(|preset| keys.iter().filter_map(|k| pack(k)).any(|p| p.presets.contains(preset)))
        .collect()
}

/// Labels the packs bring, deduplicated by key (first pack wins), with `other` last.
pub fn labels_for(keys: &[&str]) -> Vec<&'static LabelSeed> {
    let mut out: Vec<&'static LabelSeed> = Vec::new();
    for l in keys.iter().filter_map(|k| pack(k)).flat_map(|p| p.labels.iter()) {
        if l.key != OTHER_LABEL && !out.iter().any(|o| o.key == l.key) {
            out.push(l);
        }
    }
    out.push(&OTHER_SEED);
    out
}

/// Label keys these seeds would add to the workspace (not present at all, archived or not).
pub async fn missing_labels(conn: &mut SqliteConnection, workspace_id: &str, seeds: &[&LabelSeed]) -> sqlx::Result<usize> {
    let existing: Vec<String> = sqlx::query_scalar("SELECT key FROM labels WHERE workspace_id = ?1")
        .bind(workspace_id)
        .fetch_all(&mut *conn)
        .await?;
    Ok(seeds.iter().filter(|s| !existing.iter().any(|e| e == s.key)).count())
}

/// Adds labels that don't exist yet, after the existing ones. Archived labels stay archived.
/// Keeps the `other` safety net last when anything was added.
pub async fn seed_labels(conn: &mut SqliteConnection, workspace_id: &str, seeds: &[&LabelSeed]) -> sqlx::Result<u64> {
    let now = now_ms();
    let mut added = 0;
    for l in seeds {
        added += sqlx::query(
            "INSERT INTO labels (id, workspace_id, key, name, description, color, position, system, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, (SELECT COUNT(*) FROM labels WHERE workspace_id = ?2), ?7, ?8)
             ON CONFLICT(workspace_id, key) DO NOTHING",
        )
        .bind(new_id("lbl"))
        .bind(workspace_id)
        .bind(l.key)
        .bind(l.name)
        .bind(l.description)
        .bind(l.color)
        .bind(l.key == OTHER_LABEL)
        .bind(now)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    }
    if added > 0 {
        sqlx::query(
            "UPDATE labels SET position = (SELECT MAX(position) + 1 FROM labels WHERE workspace_id = ?1)
             WHERE workspace_id = ?1 AND key = ?2
               AND position < (SELECT MAX(position) FROM labels WHERE workspace_id = ?1)",
        )
        .bind(workspace_id)
        .bind(OTHER_LABEL)
        .execute(&mut *conn)
        .await?;
    }
    Ok(added)
}

/// Records the packs as enabled and adds what they need: workflow rows for staffing (enabled when
/// their label is active, as before). Labels are seeded by the caller. Idempotent.
pub async fn enable(conn: &mut SqliteConnection, workspace_id: &str, keys: &[&str]) -> sqlx::Result<()> {
    let now = now_ms();
    for key in keys {
        sqlx::query("INSERT OR IGNORE INTO workspace_packs (workspace_id, pack_key, enabled_at) VALUES (?1, ?2, ?3)")
            .bind(workspace_id)
            .bind(key)
            .bind(now)
            .execute(&mut *conn)
            .await?;
    }
    if keys.contains(&STAFFING) {
        for t in WORKFLOW_TEMPLATES {
            sqlx::query(
                "INSERT OR IGNORE INTO workflows (id, workspace_id, template_key, enabled, created_at)
                 VALUES (?1, ?2, ?3,
                         ?4 AND EXISTS (SELECT 1 FROM labels WHERE workspace_id = ?2 AND key = ?5 AND archived = 0), ?6)",
            )
            .bind(new_id("wf"))
            .bind(workspace_id)
            .bind(t.key)
            .bind(t.available)
            .bind(t.label_key)
            .bind(now)
            .execute(&mut *conn)
            .await?;
        }
    }
    crate::agent_registry::seed::seed_for_packs(conn, workspace_id, keys).await?;
    Ok(())
}

/// Turns packs off. Their labels, workflows and records stay; only features stop.
pub async fn disable(conn: &mut SqliteConnection, workspace_id: &str, keys: &[&str]) -> sqlx::Result<()> {
    for key in keys {
        sqlx::query("DELETE FROM workspace_packs WHERE workspace_id = ?1 AND pack_key = ?2")
            .bind(workspace_id)
            .bind(key)
            .execute(&mut *conn)
            .await?;
    }
    crate::agent_registry::seed::pause_for_packs_off(conn, workspace_id, keys).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{automations, domain::LABEL_TEMPLATES, labels::COLORS};

    #[test]
    fn catalogue_is_valid() {
        for p in PACKS {
            assert!(p.labels.iter().any(|l| l.key == OTHER_LABEL), "{} has no other", p.key);
            assert!(p.labels.iter().all(|l| COLORS.contains(&l.color) && l.description.len() <= 500), "{}", p.key);
            assert!(p.presets.iter().all(|k| automations::preset(k).is_some()), "{}", p.key);
            assert!(automations::DEFAULT_PRESETS.iter().all(|k| p.presets.contains(k)), "{}", p.key);
        }
        assert_eq!(pack(STAFFING).unwrap().labels.len(), LABEL_TEMPLATES[0].labels.len());
        assert_eq!(pack(STAFFING).unwrap().reply_style, DEFAULT_REPLY_STYLE);
        assert!(!NEUTRAL_REPLY_STYLE.contains("HR"));
    }

    #[test]
    fn presets_and_labels_follow_packs() {
        assert_eq!(presets_for(&["support"]), ["reply_to_questions", "chase_important"]);
        assert_eq!(presets_for(&[GENERAL, STAFFING]), automations::PRESET_KEYS);
        let keys: Vec<&str> = labels_for(&["support", "sales"]).iter().map(|l| l.key).collect();
        assert_eq!(keys, ["question", "bug_report", "complaint", "refund_request", "general", "sales_lead", "follow_up", "scheduling", "other"]);
        assert!(labels_for(&PACKS.iter().map(|p| p.key).collect::<Vec<_>>()).len() as i64 <= crate::labels::MAX_ACTIVE_LABELS);
    }

    #[test]
    fn drafter_prompt_takes_the_default_sign_off() {
        let staffing = crate::ai::openrouter::draft_system_prompt(STAFFING_SIGN_OFF);
        assert!(staffing.contains("sign off as \"HR Operations\""));
        assert!(!staffing.contains("{sign_off}") && !staffing.contains("HR team"));
        let acme = crate::ai::openrouter::draft_system_prompt("Acme \"Support\"\n");
        assert!(acme.contains("sign off as \"Acme Support\"") && !acme.contains("HR"));
        assert!(crate::ai::openrouter::draft_system_prompt(" ").contains("sign off as \"The team\""));
    }

    #[test]
    fn legacy_templates_map_to_packs() {
        assert_eq!(from_template("hr"), Some(STAFFING));
        assert_eq!(from_template("general"), Some(GENERAL));
        assert_eq!(from_template("blank"), Some(GENERAL));
        assert_eq!(from_template("x"), None);
    }
}
