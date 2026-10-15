//! The safety suite: loads the golden scenarios, builds an isolated workspace for each (its own
//! database, mock Gmail, one mailbox), runs one inbox case through the real runner and scores the
//! result against the scenario's expectations plus the safety invariants. Offline it uses a
//! scripted planner and costs nothing; live it uses the configured models. The golden tests in
//! `tests/golden_*.rs` and the in-product eval runs both call this module.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use axum::{
    Json, Router,
    body::Body,
    extract::Path,
    http::{Request, StatusCode},
    routing::{get, post, put},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use crate::{
    agents::registry::{
        routing::{Hints, MessageFacts, RoutingAgent, route},
        spec::{AgentSpec, Mode},
        store::AgentRow,
        templates::TEMPLATES,
    },
    agents::planner::{Scripted, ScriptedPlanner, call as tool, finish},
    ai::{
        AiError, DraftInput, DraftOutput, Drafter,
        fallback::TemplateDrafter,
        judge::{FixedJudge, Verdict},
    },
    build_state,
    platform::config::Config,
    platform::db,
    platform::domain::NormalizedEmail,
    http::router,
    mail::ingest,
    jobs::worker,
};
use http_body_util::BodyExt;
use serde::Deserialize;
use serde_json::{Value, json};
use tower::ServiceExt;

pub const ADMIN: &str = "dev:admin@acme.test";
const MAILBOX: &str = "me@acme.com";
const MAIN_THREAD: &str = "t-main";
const OUTCOME_CLASSES: &[&str] = &["approval", "waiting", "sent", "handoff", "no_action", "failed"];

mod scenario;
mod doubles;
mod run;
mod score;
mod report;
mod suites;

pub use scenario::Mail;
pub use scenario::Setup;
pub use scenario::Step;
pub use scenario::Expect;
pub use scenario::AgentSpecExpect;
pub use scenario::Scenario;
pub use scenario::load_scenarios;
pub use doubles::live_config_from;
pub use doubles::live_config;
pub use run::Outcome;
pub use run::run_scenario;
pub use run::LiveModels;
pub use run::RunOptions;
pub use run::run_scenario_with;
pub use score::lint;
pub use score::scorecard;
pub use report::routed_agent_name;
pub use report::label_set;
pub use report::spec_for;
pub use report::AgentScore;
pub use report::agent_scores;
pub use report::agent_scorecard;
pub use report::agent_scorecard_md;
pub use report::label_accuracy;
pub use report::label_accuracy_md;
pub use report::write_scorecard;
pub use suites::SuiteResult;
pub use suites::run_for_spec;
pub use suites::run_live;
use doubles::Calls;
use doubles::spawn_gmail;
use scenario::scripted;
use doubles::CannedDrafter;
use doubles::call;
use score::RunData;
use score::score;
use report::routing_failures;

