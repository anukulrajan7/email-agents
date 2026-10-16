//! The one way an expert gets mail sent to a correspondent: it files a typed `MessageRequest` and Meera's reply job
//! turns the requests of a message into one reply (`replies::compose`), through one live draft and the normal send
//! path. Nobody but Meera writes to a correspondent, and the requests carry facts and issue codes, never free
//! addresses: the recipient is the sender, someone already in the thread, or a person on the roster.
//!
//! This file holds the types and the pure rules (who may be written to, whose level governs). `store` holds the SQL.

use serde::{Deserialize, Serialize};

pub mod store;

/// The job key of Meera's reply case (one per message).
pub const REPLY_JOB: &str = "reply";

/// Meera's reply case, or a follow-up to a reply that already went out (`reply#2`): a sheet acknowledged after a first
/// receipt is a second mail on the same message, written by a second case.
pub fn is_reply_job_key(job: &str) -> bool {
    job == REPLY_JOB || job.starts_with("reply#")
}

pub use store::{Asker, Filed, Stored, hand_to_meera, park, unpark};

impl MessageRequest {
    /// Is this a request for missing details rather than a question about a sheet?
    pub fn is_ask(&self) -> bool {
        self.kind == MessageKind::Question && self.issues.iter().any(|i| i.code == MISSING_DATA)
    }
}

/// What a message says. Higher priority wins when several requests share a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Ack,
    Correction,
    Question,
    Answer,
    Custom,
    /// "We have it and are looking": lowest priority, always replaced by a real answer.
    Holding,
}

impl MessageKind {
    /// Correction > Question > Answer > Custom > Ack > Holding.
    pub fn priority(self) -> u8 {
        match self {
            MessageKind::Correction => 5,
            MessageKind::Question => 4,
            MessageKind::Answer => 3,
            MessageKind::Custom => 2,
            MessageKind::Ack => 1,
            MessageKind::Holding => 0,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            MessageKind::Ack => "ack",
            MessageKind::Correction => "correction",
            MessageKind::Question => "question",
            MessageKind::Answer => "answer",
            MessageKind::Custom => "custom",
            MessageKind::Holding => "holding",
        }
    }

    pub fn parse(raw: &str) -> Option<MessageKind> {
        [MessageKind::Ack, MessageKind::Correction, MessageKind::Question, MessageKind::Answer, MessageKind::Custom, MessageKind::Holding]
            .into_iter()
            .find(|k| k.as_str() == raw)
    }

    /// Kinds whose whole text comes from a template and typed facts (no model, no free words).
    pub fn is_template(self) -> bool {
        matches!(self, MessageKind::Ack | MessageKind::Correction | MessageKind::Question | MessageKind::Holding)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    pub label: String,
    pub value: String,
}

/// One thing to tell the correspondent, in the words they will read. `code` is for tests and counting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueText {
    pub code: String,
    pub text: String,
}

/// Who the reply goes to. There is no free address: the owner's copy is a reply policy, not a recipient.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "to", rename_all = "snake_case")]
pub enum Recipient {
    Sender,
    /// An address that wrote to us in this conversation.
    Participant { address: String },
    /// A person on the roster.
    Person { person_id: String },
}

/// Longest instruction an expert may attach to an Answer or Custom request.
pub const MAX_INSTRUCTIONS_CHARS: usize = 600;

/// What an expert wants said. S7 only files template kinds (`Ack`, `Correction`, `Question`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MessageRequest {
    pub to: Recipient,
    pub kind: MessageKind,
    pub facts: Vec<Fact>,
    pub issues: Vec<IssueText>,
    /// For Answer and Custom only: bounded, and shown to a model fenced as data.
    pub instructions: Option<String>,
    pub never_do: Vec<String>,
    /// Text written by an agent whose owner turned its planner on; Meera delivers it as written.
    pub body: Option<String>,
}

impl MessageRequest {
    /// A request for a template message to the sender of the email.
    pub fn template(kind: MessageKind, facts: Vec<Fact>, issues: Vec<IssueText>) -> Self {
        Self { to: Recipient::Sender, kind, facts, issues, instructions: None, never_do: vec![], body: None }
    }
}

pub use super::plan::wait::Wait;

/// A thing a sheet may lack and the sender can supply. `key` is what is stored and counted; `label` is what the sender reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Name,
    EmployeeCode,
    Email,
    Period,
}

impl Field {
    pub const ALL: [Field; 4] = [Field::Name, Field::EmployeeCode, Field::Email, Field::Period];

    pub fn key(self) -> &'static str {
        match self {
            Field::Name => "name",
            Field::EmployeeCode => "employee_code",
            Field::Email => "email",
            Field::Period => "period",
        }
    }

    pub fn parse(key: &str) -> Option<Field> {
        Field::ALL.into_iter().find(|f| f.key() == key)
    }

    /// What a person on the team reads ("did not answer: employee ID").
    pub fn short(self) -> &'static str {
        match self {
            Field::Name => "name",
            Field::EmployeeCode => "employee ID",
            Field::Email => "email address",
            Field::Period => "period",
        }
    }

    /// The words of the question, as one numbered line in the mail.
    pub fn label(self) -> &'static str {
        match self {
            Field::Name => "Your full name",
            Field::EmployeeCode => "Your employee ID",
            Field::Email => "Is the address you are writing from the right one for us to use? Reply yes, or send the right one",
            Field::Period => "The month or dates this timesheet is for",
        }
    }
}

/// The issue code that marks a question as a request for missing details.
pub const MISSING_DATA: &str = "missing_data";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Outcome {
    Done,
    NeedsPerson { why: String },
    NothingToDo,
}

/// The short note for the manager, sent only for a clean sheet and for a portal question.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ManagerSummary {
    pub headline: String,
    pub notes: Vec<String>,
}

/// What an expert decided about a message: its outcome, what to say to the sender, what to tell the manager, and what it waits for.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Decision {
    pub outcome: Outcome,
    pub message: Option<MessageRequest>,
    pub manager_summary: Option<ManagerSummary>,
    pub waits: Vec<Wait>,
    /// What to say once the waits are over (the acknowledgement of a sheet whose person is not on the roster yet). It is
    /// kept, not sent: `store::park` files it as `waiting` and the desk releases it when the person is linked.
    pub parked: Option<MessageRequest>,
    /// What the sender is asked for (the request is a Question whose issue is `MISSING_DATA`).
    pub asks: Vec<Field>,
}

/// Who wrote the words of a reply, which decides whose level governs the send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Author {
    /// A template and typed facts: the risk sits with the experts who asked.
    Template,
    /// Meera wrote it with a model: her own level counts too.
    Meera,
    /// A planner-on expert wrote the body: it is the requester, so its level is the one.
    Requester,
}

/// The level a Meera send runs at: the lowest of the requesters' levels; for model-written text also Meera's own.
/// No requester at all means nobody vouches for the words, so 0.
pub fn governing_level(requester_levels: &[i64], author: Author, meera_level: i64) -> i64 {
    let asked = requester_levels.iter().copied().min().unwrap_or(0);
    match author {
        Author::Template | Author::Requester => asked,
        Author::Meera => asked.min(meera_level),
    }
    .clamp(0, 3)
}

/// Why a request to write to this recipient is refused, if it is. The sender is always fine; a participant must have
/// written in this conversation; a person must be on the roster. An address found only in inbound text is neither.
pub fn recipient_refusal(to: &Recipient, thread_senders: &[String], roster_people: &[String]) -> Option<&'static str> {
    match to {
        Recipient::Sender => None,
        Recipient::Participant { address } => {
            let wanted = address.trim().to_lowercase();
            (!thread_senders.iter().any(|s| s.trim().to_lowercase() == wanted)).then_some("the address is not in this conversation")
        }
        Recipient::Person { person_id } => (!roster_people.iter().any(|p| p == person_id)).then_some("the person is not on the roster"),
    }
}

/// Mail signals that mean no human is waiting for a reply.
const MACHINE_SIGNALS: [&str; 7] = ["list_unsubscribe", "precedence", "auto_submitted", "auto_reply", "delivery_status", "no_reply_sender", "postroom"];

/// Why nobody should be written to for this inbound message, if so: our own mail, bulk or automatic mail, a no-reply sender,
/// or a sender that is one of our own addresses (`own`: the connected mailbox and whoever connected it).
pub fn no_reply_reason(sender: &str, signals: &[String], own: &[String]) -> Option<&'static str> {
    let bare = crate::replies::send_safety::bare_address(sender).to_lowercase();
    if own.iter().any(|o| crate::replies::send_safety::bare_address(o).to_lowercase() == bare) {
        return Some("the sender is one of our own addresses");
    }
    if crate::mail::system_mail::is_own(signals) {
        return Some("it is Post Room's own mail");
    }
    if signals.iter().any(|s| MACHINE_SIGNALS.contains(&s.as_str())) {
        return Some("it is automatic or bulk mail");
    }
    crate::replies::send_safety::is_automated_address(sender).then_some("the sender is a no-reply address")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|i| (*i).to_string()).collect()
    }

    #[test]
    fn a_template_reply_is_governed_by_the_lowest_requester_and_not_by_meera() {
        assert_eq!(governing_level(&[3], Author::Template, 0), 3, "Meera at 0 does not hold back a template");
        assert_eq!(governing_level(&[3, 1], Author::Template, 3), 1);
        assert_eq!(governing_level(&[], Author::Template, 3), 0, "nobody asked, nobody vouches");
    }

    #[test]
    fn meera_written_text_is_also_held_to_her_own_level() {
        assert_eq!(governing_level(&[3], Author::Meera, 1), 1);
        assert_eq!(governing_level(&[2], Author::Meera, 3), 2);
        assert_eq!(governing_level(&[2], Author::Requester, 0), 2, "a planner-on expert's own words use its level");
    }

    #[test]
    fn an_address_found_only_in_inbound_text_is_refused() {
        let thread = s(&["ravi@client.test"]);
        assert_eq!(recipient_refusal(&Recipient::Sender, &thread, &[]), None);
        assert_eq!(recipient_refusal(&Recipient::Participant { address: "Ravi@Client.test".into() }, &thread, &[]), None);
        assert!(recipient_refusal(&Recipient::Participant { address: "x@evil.test".into() }, &thread, &[]).is_some());
        assert!(recipient_refusal(&Recipient::Person { person_id: "p1".into() }, &thread, &s(&["p2"])).is_some());
        assert_eq!(recipient_refusal(&Recipient::Person { person_id: "p1".into() }, &thread, &s(&["p1"])), None);
    }

    #[test]
    fn nobody_is_written_to_for_own_bulk_or_no_reply_mail() {
        assert!(no_reply_reason("ravi@client.test", &s(&["postroom"]), &[]).is_some());
        assert!(no_reply_reason("ravi@client.test", &s(&["list_unsubscribe"]), &[]).is_some());
        assert!(no_reply_reason("noreply@client.test", &[], &[]).is_some());
        assert_eq!(no_reply_reason("ravi@client.test", &s(&["gmail_category_none"]), &[]), None);
        assert!(no_reply_reason("Priya <Priya@Acme.com>", &[], &s(&["priya@acme.com"])).is_some(), "our own address is never written to");
    }

    #[test]
    fn a_follow_up_reply_case_is_a_reply_job_too() {
        assert!(is_reply_job_key("reply") && is_reply_job_key("reply#2"));
        assert!(!is_reply_job_key("desk:timesheet") && !is_reply_job_key("main") && !is_reply_job_key("replying"));
    }

    #[test]
    fn kinds_rank_correction_over_question_over_ack_over_holding() {
        let mut kinds = vec![MessageKind::Holding, MessageKind::Ack, MessageKind::Correction, MessageKind::Custom, MessageKind::Question, MessageKind::Answer];
        kinds.sort_by_key(|k| std::cmp::Reverse(k.priority()));
        assert_eq!(kinds, [MessageKind::Correction, MessageKind::Question, MessageKind::Answer, MessageKind::Custom, MessageKind::Ack, MessageKind::Holding]);
        assert_eq!(MessageKind::parse("ack"), Some(MessageKind::Ack));
        assert!(MessageKind::Ack.is_template() && !MessageKind::Answer.is_template());
    }
}
