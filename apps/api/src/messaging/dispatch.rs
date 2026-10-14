//! One place that turns "send this to them" into stored, audited, held messages: an email from the person's
//! mailbox, in-app messages to members, with an optional watch (ask and report back) and an optional support
//! ticket. Callers decide *whether* it may go (a person's confirmation, or `rules`); this decides *how*.

use serde_json::{Value, json};

use super::{
    email::{self, NewEmail},
    inapp::{self, NewMessage},
    recipients::{Kind, Person, Target},
    support::{self, NewTicket},
    watch::{self, NewWatch},
};
use crate::{
    agents::{sentence, triggers},
    audit::{self, Actor, Entry},
    db::now_ms,
    error::{AppError, AppResult},
    http::AppState,
};

pub const MAX_RECIPIENTS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Email,
    InApp,
}

pub struct WatchSpec<'a> {
    pub question: &'a str,
    pub nudge_hours: i64,
}

pub struct SupportSpec<'a> {
    pub company: &'a str,
    pub address_source: &'a str,
    pub source_message_id: Option<&'a str>,
}

pub struct Request<'a> {
    pub ws: &'a str,
    /// The person Meera works for: the mailbox the email goes from and whose conversation hears about it.
    pub owner: Option<&'a str>,
    pub agent_id: Option<&'a str>,
    pub channel: Channel,
    pub target: &'a Target,
    pub subject: &'a str,
    pub body: &'a str,
    /// The person who confirmed this send. `None` = nobody asked: the send is automatic and held `hold_secs`.
    pub approver: Option<&'a str>,
    pub hold_secs: i64,
    pub watch: Option<WatchSpec<'a>>,
    pub support: Option<SupportSpec<'a>>,
    pub via: &'a str,
}

#[derive(Debug, Clone)]
pub struct Dispatched {
    pub summary: String,
    pub link: Value,
    pub watch_id: Option<String>,
    pub ticket_id: Option<String>,
    pub send_at_ms: Option<i64>,
}

fn when(send_at_ms: i64) -> String {
    let secs = ((send_at_ms - now_ms()) / 1000).max(0);
    match secs {
        0..=1 => "right now".into(),
        2..=89 => format!("in {secs} seconds"),
        _ => format!("in {} minutes", (secs + 30) / 60),
    }
}

/// Sends (holds) the message to everyone in the target.
pub async fn send(state: &AppState, req: Request<'_>) -> AppResult<Dispatched> {
    let people: Vec<&Person> = req.target.people();
    if people.is_empty() {
        return Err(AppError::BadRequest("there is nobody to send it to".into()));
    }
    if people.len() > MAX_RECIPIENTS {
        return Err(AppError::BadRequest(format!("a message can go to at most {MAX_RECIPIENTS} people")));
    }
    let channel = if matches!(req.target, Target::Team { .. }) { Channel::InApp } else { req.channel };
    match channel {
        Channel::Email => send_email(state, &req, people[0]).await,
        Channel::InApp => send_in_app(state, &req, &people).await,
    }
}

async fn send_email(state: &AppState, req: &Request<'_>, person: &Person) -> AppResult<Dispatched> {
    let Some(address) = person.address.as_deref() else {
        return Err(AppError::BadRequest(format!("I do not have an email address for {}.", person.label)));
    };
    let actor = req.approver.map_or(Actor::System, Actor::User);
    let row = email::create(
        state,
        NewEmail {
            ws: req.ws,
            owner_user_id: req.owner,
            agent_id: req.agent_id,
            to_address: address,
            to_label: &person.label,
            kind: person.kind,
            subject: req.subject,
            body: req.body,
            authored_by: "agent",
            approval_reason: None,
            in_reply_to: None,
            provider_thread_id: None,
        },
    )
    .await?;
    let mut watch_id = None;
    let mut ticket_id = None;
    if let (Some(spec), Some(owner)) = (&req.watch, req.owner) {
        let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
        let id = watch::create(
            &mut tx,
            NewWatch {
                ws: req.ws,
                user_id: owner,
                agent_id: req.agent_id,
                kind: if req.support.is_some() { "ticket" } else { "ask" },
                title: req.support.as_ref().map_or(req.subject, |_| req.subject),
                channel: "email",
                counterpart: req.support.as_ref().map_or(person.label.as_str(), |s| s.company),
                outbound_email_id: Some(&row.id),
                thread_key: None,
                source_id: Some(&row.source_id),
                case_id: None,
                question: spec.question,
                nudge_hours: spec.nudge_hours,
            },
        )
        .await?;
        if let Some(s) = &req.support {
            ticket_id = Some(
                support::create_ticket(
                    &mut tx,
                    NewTicket {
                        ws: req.ws,
                        user_id: owner,
                        watch_id: &id,
                        company: s.company,
                        address,
                        address_source: s.address_source,
                        subject: req.subject,
                        source_message_id: s.source_message_id,
                    },
                )
                .await?,
            );
        }
        tx.commit().await?;
        watch_id = Some(id);
    }
    let scheduled = match email::schedule(state, req.ws, &row.id, actor, req.hold_secs, req.approver.is_none()).await {
        Ok(s) => s,
        Err(e) => {
            discard(state, req.ws, &row.id, watch_id.as_deref()).await;
            return Err(e);
        }
    };
    let held = if req.approver.is_none() { " You can stop it before then." } else { " You can still stop it until then." };
    Ok(Dispatched {
        summary: format!("Sending “{}” to {} {}.{held}", sentence(req.subject, 60), row.to_label_or_address(), when(scheduled.send_at_ms)),
        link: json!({ "kind": "sent", "id": row.id }),
        watch_id,
        ticket_id,
        send_at_ms: Some(scheduled.send_at_ms),
    })
}

async fn discard(state: &AppState, ws: &str, outbound_id: &str, watch_id: Option<&str>) {
    let _ = sqlx::query("UPDATE outbound_emails SET status = 'cancelled', updated_at = ?1 WHERE id = ?2 AND workspace_id = ?3 AND status = 'awaiting_approval'")
        .bind(now_ms())
        .bind(outbound_id)
        .bind(ws)
        .execute(&state.db)
        .await;
    if let Some(w) = watch_id {
        let _ = sqlx::query("UPDATE watches SET status = 'closed' WHERE id = ?1 AND workspace_id = ?2").bind(w).bind(ws).execute(&state.db).await;
        let _ = sqlx::query("DELETE FROM support_tickets WHERE watch_id = ?1 AND workspace_id = ?2").bind(w).bind(ws).execute(&state.db).await;
    }
}

async fn send_in_app(state: &AppState, req: &Request<'_>, people: &[&Person]) -> AppResult<Dispatched> {
    let mut members: Vec<(&Person, &str)> = vec![];
    for p in people {
        match p.user_id.as_deref() {
            Some(u) => members.push((p, u)),
            None => return Err(AppError::BadRequest(format!("{} has no account here, so I cannot message them in the app. Email them instead.", p.label))),
        }
    }
    if members.iter().any(|(_, u)| Some(*u) == req.owner) && members.len() == 1 {
        return Err(AppError::BadRequest("that would message yourself".into()));
    }
    let deliver_at = now_ms() + if req.approver.is_none() { req.hold_secs.max(0) * 1000 } else { 0 };
    let subject = req.subject.trim();
    let team_id = match req.target {
        Target::Team { id, .. } => Some(id.as_str()),
        Target::Person(_) => None,
    };
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let mut thread_key: Option<String> = None;
    let mut first_id = String::new();
    for (person, user) in &members {
        let (id, key) = inapp::insert(
            &mut tx,
            NewMessage {
                ws: req.ws,
                thread_key: thread_key.as_deref(),
                sender_user_id: req.owner,
                sender_agent_id: req.agent_id,
                recipient: user,
                team_id,
                kind: "message",
                subject,
                body: req.body,
                link: Value::Null,
                via: req.via,
                deliver_at,
            },
        )
        .await?;
        if first_id.is_empty() {
            first_id = id.clone();
        }
        thread_key.get_or_insert(key);
        audit::record(
            &mut tx,
            Entry {
                workspace_id: req.ws,
                actor: req.approver.map_or(Actor::System, Actor::User),
                action: "message.in_app_sent",
                entity_type: "in_app_message",
                entity_id: &id,
                message_id: None,
                metadata: json!({ "recipient_kind": person.kind.as_str(), "auto": req.approver.is_none(), "via": req.via, "team": team_id.is_some() }),
            },
        )
        .await?;
    }
    if req.approver.is_none() {
        audit::record(
            &mut tx,
            Entry {
                workspace_id: req.ws,
                actor: Actor::System,
                action: email::AUTO_SENT_ACTION,
                entity_type: "in_app_message",
                entity_id: &first_id,
                message_id: None,
                metadata: json!({ "channel": "in_app", "owner": req.owner, "hold_secs": req.hold_secs }),
            },
        )
        .await?;
    }
    let key = thread_key.unwrap_or_default();
    let mut watch_id = None;
    if let (Some(spec), Some(owner)) = (&req.watch, req.owner) {
        watch_id = Some(
            watch::create(
                &mut tx,
                NewWatch {
                    ws: req.ws,
                    user_id: owner,
                    agent_id: req.agent_id,
                    kind: "ask",
                    title: subject,
                    channel: "in_app",
                    counterpart: &req.target.label(),
                    outbound_email_id: None,
                    thread_key: Some(&key),
                    source_id: None,
                    case_id: None,
                    question: spec.question,
                    nudge_hours: spec.nudge_hours,
                },
            )
            .await?,
        );
    }
    tx.commit().await?;
    let who = req.target.label();
    let held = deliver_at > now_ms();
    let summary = if held {
        format!("I will message {who} {}. You can stop it before then.", when(deliver_at))
    } else {
        format!("I sent your message to {who}.")
    };
    if let Some(owner) = req.owner {
        let text = if held {
            format!("I am about to send “{}” to {who} ({}). You can stop it until then.", sentence(subject, 80), when(deliver_at))
        } else {
            format!("I sent “{}” to {who}.", sentence(subject, 80))
        };
        let links = json!([{ "kind": "sent", "id": first_id, "label": "See what I sent" }]);
        if let Err(e) = super::report(state, req.ws, owner, &text, &[], &links).await {
            tracing::warn!(error = %e, "could not tell the person about the in-app message");
        }
    }
    Ok(Dispatched { summary, link: json!({ "kind": "sent", "id": first_id }), watch_id, ticket_id: None, send_at_ms: held.then_some(deliver_at) })
}

/// Hands an email to the desk that owns it (routing picks the agent), and watches the case so the person hears the outcome.
pub async fn hand_to_desk(state: &AppState, ws: &str, user_id: &str, message_id: &str, note: &str, nudge_hours: i64) -> AppResult<Dispatched> {
    let started = triggers::start_inbox_case(state, ws, message_id, Some(user_id)).await?;
    let (case_id, _created) = match started {
        triggers::Started::Case(id, created) => (id, created),
        triggers::Started::NotFound => return Err(AppError::NotFound),
        triggers::Started::NotAllowed(why) => return Err(AppError::Conflict(why)),
    };
    let desk: Option<(String, String)> = sqlx::query_as(
        "SELECT a.name, m.subject FROM cases c JOIN agents a ON a.id = c.agent_id JOIN email_messages m ON m.id = c.message_id WHERE c.id = ?1 AND c.workspace_id = ?2",
    )
    .bind(&case_id)
    .bind(ws)
    .fetch_optional(&state.db)
    .await?;
    let (agent_name, subject) = desk.unwrap_or_else(|| ("the team".into(), "this email".into()));
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let existing: Option<String> =
        sqlx::query_scalar("SELECT id FROM watches WHERE workspace_id = ?1 AND user_id = ?2 AND case_id = ?3 AND status != 'closed'").bind(ws).bind(user_id).bind(&case_id).fetch_optional(&mut *tx).await?;
    let watch_id = match existing {
        Some(id) => id,
        None => {
            watch::create(
                &mut tx,
                NewWatch {
                    ws,
                    user_id,
                    agent_id: None,
                    kind: "desk",
                    title: &subject,
                    channel: "desk",
                    counterpart: &agent_name,
                    outbound_email_id: None,
                    thread_key: None,
                    source_id: None,
                    case_id: Some(&case_id),
                    question: note,
                    nudge_hours,
                },
            )
            .await?
        }
    };
    audit::record(
        &mut tx,
        Entry {
            workspace_id: ws,
            actor: Actor::User(user_id),
            action: "message.handed_to_desk",
            entity_type: "case",
            entity_id: &case_id,
            message_id: Some(message_id),
            metadata: json!({ "watch_id": watch_id }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Dispatched {
        summary: format!("I handed “{}” to {agent_name}. I will tell you what happens.", sentence(&subject, 60)),
        link: json!({ "kind": "case", "id": case_id }),
        watch_id: Some(watch_id),
        ticket_id: None,
        send_at_ms: None,
    })
}

/// What kind of recipient does an address belong to? (For callers that only have an address.)
pub async fn kind_of(state: &AppState, ws: &str, address: &str) -> sqlx::Result<Kind> {
    Ok(super::recipients::classify(&state.db, ws, address).await?.kind)
}
