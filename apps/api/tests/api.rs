//! Every integration test lives in this one binary, so the crate links once and not once per file.
//! The files stay where they are; each is a module here. A new `tests/<name>.rs` needs one line
//! below (`every_test_file_is_part_of_this_binary` fails otherwise).

mod common;
mod staffing_common;
#[path = "golden/harness.rs"]
mod golden_harness;

#[path = "agent_profiles.rs"]
mod agent_profiles;
#[path = "agent_registry.rs"]
mod agent_registry;
#[path = "agents.rs"]
mod agents;
#[path = "ai_incidents.rs"]
mod ai_incidents;
#[path = "ai_usage.rs"]
mod ai_usage;
#[path = "alerts.rs"]
mod alerts;
#[path = "assistant.rs"]
mod assistant;
#[path = "attachments.rs"]
mod attachments;
#[path = "auth.rs"]
mod auth;
#[path = "automations.rs"]
mod automations;
#[path = "autonomy_levels.rs"]
mod autonomy_levels;
#[path = "bulk_filter.rs"]
mod bulk_filter;
#[path = "candidate_watch.rs"]
mod candidate_watch;
#[path = "db_concurrency.rs"]
mod db_concurrency;
#[path = "deletion.rs"]
mod deletion;
#[path = "designer.rs"]
mod designer;
#[path = "experts.rs"]
mod experts;
#[path = "flags.rs"]
mod flags;
#[path = "gmail_backfill.rs"]
mod gmail_backfill;
#[path = "gmail_direction.rs"]
mod gmail_direction;
#[path = "golden_drafts.rs"]
mod golden_drafts;
#[path = "golden_live.rs"]
mod golden_live;
#[path = "golden_offline.rs"]
mod golden_offline;
#[path = "golden_understanding.rs"]
mod golden_understanding;
#[path = "inbox_insights.rs"]
mod inbox_insights;
#[path = "knowledge.rs"]
mod knowledge;
#[path = "label_health.rs"]
mod label_health;
#[path = "labels.rs"]
mod labels;
#[path = "learning.rs"]
mod learning;
#[path = "learning_guard.rs"]
mod learning_guard;
#[path = "matching.rs"]
mod matching;
#[path = "matching_semantic.rs"]
mod matching_semantic;
#[path = "memory.rs"]
mod memory;
#[path = "memory_learning.rs"]
mod memory_learning;
#[path = "messaging.rs"]
mod messaging;
#[path = "metrics_evals.rs"]
mod metrics_evals;
#[path = "migrate_from_0033.rs"]
mod migrate_from_0033;
#[path = "model_body.rs"]
mod model_body;
#[path = "nango.rs"]
mod nango;
#[path = "one_live_draft.rs"]
mod one_live_draft;
#[path = "packs.rs"]
mod packs;
#[path = "pipeline.rs"]
mod pipeline;
#[path = "reply_settings.rs"]
mod reply_settings;
#[path = "routing.rs"]
mod routing;
#[path = "security.rs"]
mod security;
#[path = "send_safety.rs"]
mod send_safety;
#[path = "single_flight.rs"]
mod single_flight;
#[path = "staffing_e2e.rs"]
mod staffing_e2e;
#[path = "staffing_escalation.rs"]
mod staffing_escalation;
#[path = "staffing_metrics.rs"]
mod staffing_metrics;
#[path = "staffing_packs.rs"]
mod staffing_packs;
#[path = "staffing_people.rs"]
mod staffing_people;
#[path = "staffing_policy.rs"]
mod staffing_policy;
#[path = "staffing_portal.rs"]
mod staffing_portal;
#[path = "staffing_roles.rs"]
mod staffing_roles;
#[path = "staffing_round2.rs"]
mod staffing_round2;
#[path = "staffing_scope.rs"]
mod staffing_scope;
#[path = "staffing_submissions.rs"]
mod staffing_submissions;
#[path = "summaries.rs"]
mod summaries;
#[path = "system_mail.rs"]
mod system_mail;
#[path = "tenant_isolation.rs"]
mod tenant_isolation;
#[path = "timesheet_desk.rs"]
mod timesheet_desk;
#[path = "timesheet_records.rs"]
mod timesheet_records;
#[path = "understanding_v2.rs"]
mod understanding_v2;
#[path = "workflow_gate.rs"]
mod workflow_gate;

#[test]
fn every_test_file_is_part_of_this_binary() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let api = include_str!("api.rs");
    let manifest = include_str!("../Cargo.toml");
    let mut missing = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".rs").filter(|s| *s != "api") else { continue };
        let as_module = api.contains(&format!("#[path = \"{name}\"]\nmod {stem};"));
        let as_target = manifest.contains(&format!("path = \"tests/{name}\""));
        if !as_module && !as_target {
            missing.push(name);
        }
    }
    assert!(missing.is_empty(), "add these to tests/api.rs (or a [[test]] entry in Cargo.toml), or they never run: {missing:?}");
}
