//! Who owns which job of a message. This is the timesheet path only (slice S6): a timesheet email has one job, `main`,
//! and the timesheet desk (Rohan) owns it at every autonomy level. The other jobs and mail kinds come in later slices.
//! `decide_pure` has no I/O; `decide` gathers its facts; every decision is written to the audit trail, as
//! `ownership.decided` when it acts and `ownership.compare` when the flag is only comparing.

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::SqliteConnection;

use super::registry::{routing, store, templates};
use crate::{
    hr::staffing::timesheet_records::LABEL as TIMESHEET_LABEL,
    http::AppState,
    platform::{
        audit::{self, Actor, Entry},
        packs,
    },
};

pub const MAIN: &str = "main";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Why {
    NotInbound,
    SystemMail,
    HeldBack,
    NotTimesheet,
    NotRecent,
    RoutedElsewhere,
    NoDesk,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub agent_id: String,
    pub name: String,
    pub template_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub key: &'static str,
    pub agent_id: String,
    pub agent_name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Owner {
    Nobody(Why),
    Assigned(Vec<Job>),
}

pub struct Facts<'a> {
    pub inbound: bool,
    pub own_mail: bool,
    pub suppressed: bool,
    pub label: Option<&'a str>,
    pub staffing: bool,
    pub recent: bool,
    /// The agent routing picked, and whether a person chose it by hand.
    pub routed: Option<(Candidate, bool)>,
    /// The workspace's active timesheet desk.
    pub desk: Option<Candidate>,
}

pub fn decide_pure(f: &Facts) -> Owner {
    if !f.inbound {
        return Owner::Nobody(Why::NotInbound);
    }
    if f.own_mail {
        return Owner::Nobody(Why::SystemMail);
    }
    if f.suppressed {
        return Owner::Nobody(Why::HeldBack);
    }
    if f.label != Some(TIMESHEET_LABEL) || !f.staffing {
        return Owner::Nobody(Why::NotTimesheet);
    }
    if !f.recent {
        return Owner::Nobody(Why::NotRecent);
    }
    let is_desk = |c: &Candidate| c.template_key.as_deref() == Some(templates::TIMESHEETS_KEY);
    let owner = match &f.routed {
        Some((c, _)) if is_desk(c) => Some(c),
        Some((_, true)) => return Owner::Nobody(Why::RoutedElsewhere),
        _ => f.desk.as_ref(),
    };
    match owner {
        Some(c) => Owner::Assigned(vec![Job { key: MAIN, agent_id: c.agent_id.clone(), agent_name: c.name.clone() }]),
        None => Owner::Nobody(Why::NoDesk),
    }
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    direction: String,
    suppressed: bool,
    received_at: i64,
    category: Option<String>,
}

pub async fn decide(state: &AppState, ws: &str, message_id: &str) -> sqlx::Result<Owner> {
    let row: Option<MessageRow> = sqlx::query_as(
        "SELECT m.direction, m.suppressed, m.received_at, c.category FROM email_messages m
         LEFT JOIN classifications c ON c.message_id = m.id WHERE m.id = ?1 AND m.workspace_id = ?2",
    )
    .bind(message_id)
    .bind(ws)
    .fetch_optional(&state.db)
    .await?;
    let Some(m) = row else { return Ok(Owner::Nobody(Why::NotInbound)) };
    let staffing = packs::enabled(&state.db, ws, packs::STAFFING).await?;
    let candidate = |a: &routing::RoutingAgent| Candidate { agent_id: a.row.id.clone(), name: a.row.name.clone(), template_key: a.row.template_key.clone() };
    let routed = if m.category.as_deref() == Some(TIMESHEET_LABEL) && staffing {
        routing::route_message(&state.db, &state.config, ws, message_id).await?.map(|r| (candidate(&r.primary), r.route.source == routing::Source::Manual))
    } else {
        None
    };
    let desk = if staffing { store::by_template(&state.db, ws, templates::TIMESHEETS_KEY).await?.filter(|a| a.status == "active") } else { None };
    let facts = Facts {
        inbound: m.direction == "inbound",
        own_mail: crate::mail::system_mail::is_own_message(&state.db, ws, message_id).await?,
        suppressed: m.suppressed,
        label: m.category.as_deref(),
        staffing,
        recent: m.received_at >= crate::platform::db::now_ms() - state.config.ai_auto_hours * 3_600_000,
        routed,
        desk: desk.map(|a| Candidate { agent_id: a.id, name: a.name, template_key: a.template_key }),
    };
    Ok(decide_pure(&facts))
}

/// `ownership.decided` (the owner acts) or `ownership.compare` (the flag only compares; `extra` carries what it would do).
pub async fn audit_owner(conn: &mut SqliteConnection, ws: &str, message_id: &str, action: &str, owner: &Owner, extra: Value) -> sqlx::Result<()> {
    let mut metadata = match owner {
        Owner::Assigned(jobs) => json!({ "jobs": jobs.iter().map(|j| json!({"job": j.key, "agent_id": j.agent_id})).collect::<Vec<_>>() }),
        Owner::Nobody(why) => json!({ "nobody": why }),
    };
    if let (Some(base), Some(add)) = (metadata.as_object_mut(), extra.as_object()) {
        base.extend(add.clone());
    }
    audit::record(conn, Entry { workspace_id: ws, actor: Actor::System, action, entity_type: "message", entity_id: message_id, message_id: Some(message_id), metadata }).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rohan() -> Candidate {
        Candidate { agent_id: "a1".into(), name: "Rohan".into(), template_key: Some(templates::TIMESHEETS_KEY.into()) }
    }

    fn lead() -> Candidate {
        Candidate { agent_id: "a2".into(), name: "Meera".into(), template_key: Some(templates::LEAD_KEY.into()) }
    }

    fn facts() -> Facts<'static> {
        Facts { inbound: true, own_mail: false, suppressed: false, label: Some("timesheet"), staffing: true, recent: true, routed: Some((rohan(), false)), desk: Some(rohan()) }
    }

    fn assigned(o: Owner) -> Vec<(&'static str, String)> {
        match o {
            Owner::Assigned(jobs) => jobs.into_iter().map(|j| (j.key, j.agent_name)).collect(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_timesheet_goes_to_the_desk_as_the_main_job() {
        assert_eq!(assigned(decide_pure(&facts())), [("main", "Rohan".to_string())]);
    }

    #[test]
    fn the_desk_keeps_the_job_when_routing_picked_someone_else_by_rule_not_by_hand() {
        let f = Facts { routed: Some((lead(), false)), ..facts() };
        assert_eq!(assigned(decide_pure(&f)), [("main", "Rohan".to_string())]);
        let by_hand = Facts { routed: Some((lead(), true)), ..facts() };
        assert_eq!(decide_pure(&by_hand), Owner::Nobody(Why::RoutedElsewhere));
    }

    #[test]
    fn nobody_owns_what_is_not_a_recent_inbound_timesheet() {
        assert_eq!(decide_pure(&Facts { inbound: false, ..facts() }), Owner::Nobody(Why::NotInbound));
        assert_eq!(decide_pure(&Facts { own_mail: true, ..facts() }), Owner::Nobody(Why::SystemMail));
        assert_eq!(decide_pure(&Facts { suppressed: true, ..facts() }), Owner::Nobody(Why::HeldBack));
        assert_eq!(decide_pure(&Facts { label: Some("leave"), ..facts() }), Owner::Nobody(Why::NotTimesheet));
        assert_eq!(decide_pure(&Facts { staffing: false, ..facts() }), Owner::Nobody(Why::NotTimesheet));
        assert_eq!(decide_pure(&Facts { recent: false, ..facts() }), Owner::Nobody(Why::NotRecent));
        assert_eq!(decide_pure(&Facts { routed: None, desk: None, ..facts() }), Owner::Nobody(Why::NoDesk));
    }
}
