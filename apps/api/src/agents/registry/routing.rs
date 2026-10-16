//! Routing: which agent gets an email. No AI call; a pure function over the message's facts and
//! the workspace's active agents. Precedence: manual, thread, rule, label, personal, generic, fallback.

use sqlx::{SqliteConnection, SqlitePool};

use super::{
    spec::{AgentSpec, Trigger, TriggerMatch},
    store::{self, AgentRow},
    versions,
};
use crate::{
    agents::autonomy::{self, LevelInputs},
    platform::config::Config,
    platform::db::now_ms,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A person reassigned the message.
    Manual,
    /// An open case, or an earlier route, in the same conversation.
    Thread,
    /// A trigger on domains, addresses, keywords, attachment types or priority.
    Rule,
    /// A trigger on labels.
    Label,
    /// The mailbox owner's personal agent.
    Personal,
    /// A trigger with no conditions beyond "needs a reply".
    Generic,
    /// The lead.
    Fallback,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Manual => "manual",
            Source::Thread => "thread",
            Source::Rule => "rule",
            Source::Label => "label",
            Source::Personal => "personal",
            Source::Generic => "generic",
            Source::Fallback => "fallback",
        }
    }
}

/// An active agent as routing sees it.
#[derive(Debug, Clone)]
pub struct RoutingAgent {
    pub row: AgentRow,
    pub spec: AgentSpec,
    pub version: i64,
    /// Effective autonomy level, without the mailbox owner's profile.
    pub level: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MessageFacts {
    pub sender_address: String,
    pub subject: String,
    pub label: Option<String>,
    pub priority: Option<String>,
    pub needs_reply: bool,
    /// File extensions and media subtypes of the attachments, lower-case.
    pub attachment_types: Vec<String>,
    /// The member whose mailbox the email arrived in.
    pub owner_user_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Hints {
    /// A person's choice.
    pub manual: Option<String>,
    /// The agent that already works this conversation.
    pub thread: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// The agent the route points to.
    pub agent_id: String,
    pub source: Source,
    /// The agent is at level 0: it only observes. A real handler works the mail meanwhile.
    pub shadow: bool,
    /// The agent that does the real work (the route's agent unless it only observes).
    pub handler_id: String,
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
enum Class {
    Generic,
    Label,
    Rule,
}

/// How a trigger matches a message: its class and how many conditions it set (specificity).
fn match_trigger(m: &TriggerMatch, f: &MessageFacts) -> Option<(Class, usize)> {
    let sender = f.sender_address.trim().to_lowercase();
    let domain = sender.rsplit_once('@').map(|(_, d)| d).unwrap_or("");
    let subject = f.subject.to_lowercase();
    let mut conditions = 0;
    let mut rule_like = false;
    if !m.labels.is_empty() {
        conditions += 1;
        if !f.label.as_deref().is_some_and(|l| m.labels.iter().any(|x| x.eq_ignore_ascii_case(l))) {
            return None;
        }
    }
    if !m.from_domains.is_empty() {
        conditions += 1;
        rule_like = true;
        if !m.from_domains.iter().any(|d| domain == d || domain.ends_with(&format!(".{d}"))) {
            return None;
        }
    }
    if !m.from_addresses.is_empty() {
        conditions += 1;
        rule_like = true;
        if !m.from_addresses.iter().any(|a| a.eq_ignore_ascii_case(&sender)) {
            return None;
        }
    }
    if !m.subject_keywords.is_empty() {
        conditions += 1;
        rule_like = true;
        if !m.subject_keywords.iter().any(|k| subject.contains(&k.to_lowercase())) {
            return None;
        }
    }
    if let Some(want) = m.needs_reply {
        conditions += 1;
        if f.needs_reply != want {
            return None;
        }
    }
    if let Some(p) = &m.priority {
        conditions += 1;
        rule_like = true;
        if f.priority.as_deref() != Some(p.as_str()) {
            return None;
        }
    }
    if !m.attachment_types.is_empty() {
        conditions += 1;
        rule_like = true;
        if !m.attachment_types.iter().any(|t| f.attachment_types.iter().any(|a| a == t)) {
            return None;
        }
    }
    let class = if rule_like {
        Class::Rule
    } else if !m.labels.is_empty() {
        Class::Label
    } else {
        Class::Generic
    };
    Some((class, conditions))
}

/// The best class and specificity among an agent's email triggers that match.
fn best_match(agent: &RoutingAgent, f: &MessageFacts) -> Option<(Class, usize)> {
    agent
        .spec
        .triggers
        .iter()
        .filter_map(|t| match t {
            Trigger::EmailReceived { matcher } => match_trigger(matcher, f),
            _ => None,
        })
        .max_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)))
}

/// Picks the agent. `agents` are the workspace's active agents; `None` only when there is no lead.
pub fn route(f: &MessageFacts, hints: &Hints, agents: &[RoutingAgent]) -> Option<Route> {
    let usable = |a: &&RoutingAgent| a.row.status == "active" && (!a.row.is_personal() || a.row.owner_user_id == f.owner_user_id);
    let by_id = |id: &Option<String>| id.as_deref().and_then(|id| agents.iter().filter(usable).find(|a| a.row.id == id));
    let mut chain: Vec<(&RoutingAgent, Source)> = Vec::new();
    chain.extend(by_id(&hints.manual).map(|a| (a, Source::Manual)));
    chain.extend(by_id(&hints.thread).map(|a| (a, Source::Thread)));
    let mut matches: Vec<(&RoutingAgent, Class, usize)> = agents
        .iter()
        .filter(usable)
        .filter(|a| a.row.kind != "lead" && !a.row.is_personal())
        .filter_map(|a| best_match(a, f).map(|(c, n)| (a, c, n)))
        .collect();
    matches.sort_by(|x, y| y.1.cmp(&x.1).then(y.2.cmp(&x.2)).then(x.0.row.created_at.cmp(&y.0.row.created_at)).then(x.0.row.id.cmp(&y.0.row.id)));
    for (class, source) in [(Class::Rule, Source::Rule), (Class::Label, Source::Label)] {
        chain.extend(matches.iter().filter(|m| m.1 == class).map(|m| (m.0, source)));
    }
    if let Some(owner) = &f.owner_user_id {
        chain.extend(
            agents
                .iter()
                .filter(usable)
                .find(|a| a.row.is_personal() && a.row.owner_user_id.as_ref() == Some(owner))
                .map(|a| (a, Source::Personal)),
        );
    }
    chain.extend(matches.iter().filter(|m| m.1 == Class::Generic).map(|m| (m.0, Source::Generic)));
    let lead = agents.iter().find(|a| a.row.kind == "lead" && a.row.status == "active");
    chain.extend(lead.map(|a| (a, Source::Fallback)));

    let (primary, source) = *chain.first()?;
    let observing = primary.level == 0;
    let handler = if observing {
        lead.filter(|l| l.level >= 1)
            .or_else(|| chain.iter().map(|(a, _)| *a).find(|a| a.level >= 1 && !a.row.is_personal()))
            .or(lead)
            .unwrap_or(primary)
    } else {
        primary
    };
    Some(Route { agent_id: primary.row.id.clone(), source, shadow: primary.level == 0, handler_id: handler.row.id.clone() })
}

// ---------------------------------------------------------------- database side

/// The workspace's agents that may take work: active, with their current spec and level.
pub async fn active_agents(db: &SqlitePool, config: &Config, ws: &str) -> sqlx::Result<Vec<RoutingAgent>> {
    let mut out = Vec::new();
    let mut conn = db.acquire().await?;
    for row in store::list(&mut *conn, ws).await?.into_iter().filter(|a| a.status == "active") {
        let (spec, version) = versions::active_spec(&mut conn, &row).await?;
        let level = autonomy::effective_level(LevelInputs {
            earned: row.autonomy_level,
            cap: row.autonomy_cap,
            spec_max: i64::from(spec.permissions.max_autonomy),
            env_max: config.agent_max_autonomy,
            profile_asks_first: false,
            active: true,
        })
        .level;
        out.push(RoutingAgent { row, spec, version, level });
    }
    Ok(out)
}

#[derive(sqlx::FromRow)]
struct FactsRow {
    sender_address: String,
    subject: String,
    thread_id: String,
    category: Option<String>,
    priority: Option<String>,
    needs_reply: Option<bool>,
    owner_id: Option<String>,
}

async fn attachment_types(db: &SqlitePool, message_id: &str) -> sqlx::Result<Vec<String>> {
    let rows: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT filename, media_type FROM attachments WHERE message_id = ?1").bind(message_id).fetch_all(db).await?;
    let mut out: Vec<String> = Vec::new();
    for (filename, media) in rows {
        let ext = filename.rsplit_once('.').map(|(_, e)| e.to_lowercase());
        let subtype = media.as_deref().and_then(|m| m.rsplit_once('/')).map(|(_, s)| s.to_lowercase());
        for t in ext.into_iter().chain(subtype) {
            if !out.contains(&t) {
                out.push(t);
            }
        }
    }
    Ok(out)
}

/// Facts of one inbound message, and the conversation it belongs to.
pub async fn message_facts(db: &SqlitePool, ws: &str, message_id: &str) -> sqlx::Result<Option<(MessageFacts, String)>> {
    let row: Option<FactsRow> = sqlx::query_as(
        "SELECT m.sender_address, m.subject, m.thread_id, c.category, c.priority,
                COALESCE(i.needs_reply, c.requires_reply) AS needs_reply, u.id AS owner_id
         FROM email_messages m
         LEFT JOIN classifications c ON c.message_id = m.id
         LEFT JOIN message_insights i ON i.message_id = m.id
         LEFT JOIN email_sources s ON s.id = m.source_id
         LEFT JOIN users u ON u.id = s.connected_by_user_id
           AND EXISTS (SELECT 1 FROM memberships ms WHERE ms.workspace_id = m.workspace_id AND ms.user_id = u.id)
         WHERE m.id = ?1 AND m.workspace_id = ?2 AND m.direction = 'inbound'",
    )
    .bind(message_id)
    .bind(ws)
    .fetch_optional(db)
    .await?;
    let Some(r) = row else { return Ok(None) };
    let facts = MessageFacts {
        sender_address: r.sender_address,
        subject: r.subject,
        label: r.category,
        priority: r.priority,
        needs_reply: r.needs_reply.unwrap_or(false),
        attachment_types: attachment_types(db, message_id).await?,
        owner_user_id: r.owner_id,
    };
    Ok(Some((facts, r.thread_id)))
}

async fn hints(db: &SqlitePool, ws: &str, message_id: &str, thread_id: &str) -> sqlx::Result<Hints> {
    let manual: Option<String> =
        sqlx::query_scalar("SELECT agent_id FROM message_routes WHERE message_id = ?1 AND workspace_id = ?2 AND source = 'manual'")
            .bind(message_id)
            .bind(ws)
            .fetch_optional(db)
            .await?;
    let open_case: Option<String> = sqlx::query_scalar(
        "SELECT agent_id FROM cases WHERE workspace_id = ?1 AND thread_id = ?2 AND shadow = 0 AND agent_id IS NOT NULL
           AND status NOT IN ('done', 'failed', 'cancelled') ORDER BY created_at DESC LIMIT 1",
    )
    .bind(ws)
    .bind(thread_id)
    .fetch_optional(db)
    .await?;
    let thread = match open_case {
        Some(id) => Some(id),
        None => {
            sqlx::query_scalar(
                "SELECT r.agent_id FROM message_routes r JOIN email_messages m ON m.id = r.message_id
                 WHERE m.workspace_id = ?1 AND m.thread_id = ?2 AND m.id != ?3 AND r.shadow = 0
                 ORDER BY m.received_at DESC, m.id DESC LIMIT 1",
            )
            .bind(ws)
            .bind(thread_id)
            .bind(message_id)
            .fetch_optional(db)
            .await?
        }
    };
    Ok(Hints { manual, thread })
}

/// The decision for one message, with the agents involved.
#[derive(Debug, Clone)]
pub struct Routed {
    pub route: Route,
    pub primary: RoutingAgent,
    pub handler: RoutingAgent,
}

/// Routes an inbound message and records the route (a person's manual choice is never overwritten
/// by a computed one). `None` when the message is not an inbound email of this workspace.
pub async fn route_message(db: &SqlitePool, config: &Config, ws: &str, message_id: &str) -> sqlx::Result<Option<Routed>> {
    super::seed::ensure_workspace(db, ws).await?;
    let Some(routed) = peek(db, config, ws, message_id).await? else { return Ok(None) };
    let mut conn = db.acquire().await?;
    record(&mut conn, ws, message_id, &routed.route, routed.primary.version).await?;
    Ok(Some(routed))
}

/// The same decision as `route_message`, read-only: nothing is recorded and nothing is seeded. For compare modes.
pub async fn peek(db: &SqlitePool, config: &Config, ws: &str, message_id: &str) -> sqlx::Result<Option<Routed>> {
    let Some((facts, thread_id)) = message_facts(db, ws, message_id).await? else { return Ok(None) };
    let agents = active_agents(db, config, ws).await?;
    let hints = hints(db, ws, message_id, &thread_id).await?;
    let Some(route) = route(&facts, &hints, &agents) else { return Ok(None) };
    let find = |id: &str| agents.iter().find(|a| a.row.id == id).cloned();
    let (Some(primary), Some(handler)) = (find(&route.agent_id), find(&route.handler_id)) else { return Ok(None) };
    Ok(Some(Routed { route, primary, handler }))
}

pub async fn record(conn: &mut SqliteConnection, ws: &str, message_id: &str, route: &Route, agent_version: i64) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO message_routes (message_id, workspace_id, agent_id, agent_version, source, shadow, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(message_id) DO UPDATE SET agent_id = excluded.agent_id, agent_version = excluded.agent_version,
           source = excluded.source, shadow = excluded.shadow, created_at = excluded.created_at
         WHERE message_routes.source != 'manual' OR excluded.source = 'manual'",
    )
    .bind(message_id)
    .bind(ws)
    .bind(&route.agent_id)
    .bind(agent_version)
    .bind(route.source.as_str())
    .bind(route.shadow)
    .bind(now_ms())
    .execute(conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::registry::templates::{self, template};

    fn agent(key: &str, id: &str, level: i64, created_at: i64) -> RoutingAgent {
        let t = template(key).unwrap();
        let row = AgentRow {
            id: id.into(),
            workspace_id: "w".into(),
            key: t.spec().key,
            kind: t.kind.into(),
            scope: t.scope.into(),
            owner_user_id: None,
            category: t.category.into(),
            template_key: Some(key.into()),
            template_version: Some(1),
            cloned_from: None,
            name: t.name.into(),
            avatar: t.avatar.into(),
            color: t.color.into(),
            description: String::new(),
            status: "active".into(),
            paused_reason: None,
            active_version: Some(1),
            tuned: false,
            autonomy_level: level,
            autonomy_cap: 3,
            next_run_at: None,
            created_by: None,
            created_at,
            updated_at: created_at,
        };
        RoutingAgent { row, spec: t.spec(), version: 1, level }
    }

    fn personal(id: &str, owner: &str) -> RoutingAgent {
        let mut a = agent(templates::PERSONAL_KEY, id, 1, 5);
        a.row.owner_user_id = Some(owner.into());
        a
    }

    fn facts(label: &str) -> MessageFacts {
        MessageFacts {
            sender_address: "cust@client.com".into(),
            subject: "Help with my order".into(),
            label: Some(label.into()),
            needs_reply: true,
            ..Default::default()
        }
    }

    fn roster() -> Vec<RoutingAgent> {
        vec![
            agent(templates::LEAD_KEY, "lead", 1, 1),
            agent(templates::INBOX_KEY, "inbox", 1, 2),
            agent("support.resolver", "resolver", 1, 3),
            agent("support.escalation", "escalation", 1, 4),
            personal("pa_alice", "alice"),
        ]
    }

    fn pick(f: &MessageFacts, hints: &Hints, agents: &[RoutingAgent]) -> (String, Source) {
        let r = route(f, hints, agents).unwrap();
        (r.agent_id, r.source)
    }

    #[test]
    fn precedence_is_manual_thread_rule_label_personal_generic_fallback() {
        let agents = roster();
        let mut f = facts("question");
        f.owner_user_id = Some("alice".into());
        let none = Hints::default();
        assert_eq!(pick(&f, &none, &agents), ("resolver".into(), Source::Label), "a label trigger beats the personal agent");
        let both = Hints { manual: Some("escalation".into()), thread: Some("resolver".into()) };
        assert_eq!(pick(&f, &both, &agents), ("escalation".into(), Source::Manual), "manual beats thread");
        let thread = Hints { manual: None, thread: Some("inbox".into()) };
        assert_eq!(pick(&f, &thread, &agents), ("inbox".into(), Source::Thread), "thread beats rules and labels");
        let mut keyworded = facts("question");
        keyworded.subject = "This is unacceptable".into();
        assert_eq!(pick(&keyworded, &none, &agents), ("resolver".into(), Source::Rule), "a keyword rule beats a label trigger");
        let plain = facts("general");
        assert_eq!(pick(&plain, &none, &agents), ("inbox".into(), Source::Generic), "no owner: the generic trigger");
        let mut owned = facts("general");
        owned.owner_user_id = Some("alice".into());
        assert_eq!(pick(&owned, &none, &agents), ("pa_alice".into(), Source::Personal), "the owner's personal agent");
        let mut other = owned.clone();
        other.owner_user_id = Some("bob".into());
        assert_eq!(pick(&other, &none, &agents), ("inbox".into(), Source::Generic), "never someone else's personal agent");
        let mut quiet = facts("general");
        quiet.needs_reply = false;
        assert_eq!(pick(&quiet, &none, &agents), ("lead".into(), Source::Fallback), "nothing matches: the lead");
    }

    #[test]
    fn a_hint_pointing_at_someone_elses_personal_agent_is_ignored() {
        let agents = roster();
        let mut f = facts("general");
        f.owner_user_id = Some("bob".into());
        let hints = Hints { manual: Some("pa_alice".into()), thread: None };
        assert_ne!(pick(&f, &hints, &agents).0, "pa_alice");
    }

    #[test]
    fn ties_go_to_the_more_specific_trigger_then_the_oldest_agent() {
        let mut agents = roster();
        let mut a = agent("sales.qualifier", "q_new", 1, 20);
        let mut b = agent("sales.followups", "q_old", 1, 10);
        for x in [&mut a, &mut b] {
            x.spec.triggers = vec![Trigger::EmailReceived { matcher: TriggerMatch { labels: vec!["sales_lead".into()], ..Default::default() } }];
        }
        agents.extend([a, b]);
        let f = facts("sales_lead");
        assert_eq!(pick(&f, &Hints::default(), &agents), ("q_old".into(), Source::Label), "oldest agent on a tie");
        agents.iter_mut().find(|x| x.row.id == "q_new").unwrap().spec.triggers = vec![Trigger::EmailReceived {
            matcher: TriggerMatch { labels: vec!["sales_lead".into()], needs_reply: Some(true), ..Default::default() },
        }];
        assert_eq!(pick(&f, &Hints::default(), &agents), ("q_new".into(), Source::Label), "more conditions win");
    }

    #[test]
    fn domains_addresses_priority_and_attachments_match() {
        let mut desk = agent("finance.invoices", "fin", 1, 3);
        desk.spec.triggers = vec![Trigger::EmailReceived {
            matcher: TriggerMatch { from_domains: vec!["vendor.com".into()], attachment_types: vec!["pdf".into()], ..Default::default() },
        }];
        let agents = vec![agent(templates::LEAD_KEY, "lead", 1, 1), desk];
        let mut f = facts("general");
        f.sender_address = "billing@eu.vendor.com".into();
        f.attachment_types = vec!["pdf".into()];
        assert_eq!(pick(&f, &Hints::default(), &agents), ("fin".into(), Source::Rule));
        f.attachment_types = vec!["png".into()];
        assert_eq!(pick(&f, &Hints::default(), &agents), ("lead".into(), Source::Fallback));
        f.attachment_types = vec!["pdf".into()];
        f.sender_address = "x@notvendor.com".into();
        assert_eq!(pick(&f, &Hints::default(), &agents).1, Source::Fallback);
    }

    #[test]
    fn an_observing_agent_is_a_shadow_and_a_real_handler_works_the_mail() {
        let mut agents = roster();
        agents.iter_mut().find(|a| a.row.id == "resolver").unwrap().level = 0;
        let r = route(&facts("question"), &Hints::default(), &agents).unwrap();
        assert_eq!((r.agent_id.as_str(), r.shadow, r.source), ("resolver", true, Source::Label));
        assert_eq!(r.handler_id, "lead", "Meera, the lead, works the mail meanwhile");
        let r = route(&facts("general"), &Hints::default(), &agents).unwrap();
        assert!(!r.shadow);
        assert_eq!(r.agent_id, r.handler_id);
    }

    #[test]
    fn paused_agents_are_not_routed_to() {
        let mut agents = roster();
        agents.iter_mut().find(|a| a.row.id == "resolver").unwrap().row.status = "paused".into();
        assert_eq!(pick(&facts("question"), &Hints::default(), &agents).0, "inbox");
        agents.retain(|a| a.row.kind != "lead");
        agents.retain(|a| a.row.id != "inbox");
        let mut quiet = facts("general");
        quiet.needs_reply = false;
        assert!(route(&quiet, &Hints::default(), &agents).is_none());
    }
}
