//! The owner's timesheet rules as a pure check. One recorded sheet goes in, with the settings and the person's
//! targets; out come typed issues and a typed decision. Nothing here reads the database or sends anything.
//!
//! Severities: a `Correction` or a `Question` is for the sender, a `Person` issue needs someone on the team, and a
//! `Note` is for the manager only and never reaches the sender.

use serde::Serialize;

use super::{CHECK, Entry, Issue, parse::MONTH_NAMES};
use crate::agents::handoff::{Decision, Fact, Field, IssueText, MISSING_DATA, ManagerSummary, MessageKind, MessageRequest, Outcome, Wait};
use crate::hr::{
    staffing::policy::MONTHS,
    team::{
        clock::{self, DAY_MS},
        settings::{Cadence, TeamSettings},
    },
};

#[derive(Debug, Clone, PartialEq)]
pub struct Rules {
    pub tolerance: f64,
    pub grace_days: i64,
    pub cadence: Cadence,
    pub max_per_day: Option<f64>,
    pub require_signature: bool,
    pub client_domains: Vec<String>,
    pub tz_offset_minutes: i64,
    /// What the sheet must state: `identity`, `name`, `employee_code`, `email`, `period`.
    pub required: Vec<String>,
}

impl Rules {
    pub fn from_settings(s: &TeamSettings) -> Rules {
        Rules {
            tolerance: s.hours_tolerance,
            grace_days: s.timesheet_grace_days,
            cadence: s.timesheet_cadence,
            max_per_day: s.max_hours_per_day,
            require_signature: s.require_signature,
            client_domains: s.client_domains.clone(),
            tz_offset_minutes: s.tz_offset_minutes,
            required: s.timesheet_required.clone(),
        }
    }
}

/// What the directory says this person is expected to do: hours a day and hours a week.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PersonRules {
    pub daily_target: Option<i64>,
    pub weekly_target: Option<i64>,
}

pub struct RecordView<'a> {
    pub period: (&'a str, &'a str),
    pub total: Option<f64>,
    pub entries: &'a [Entry],
    /// What reading the sheet found (not the rule issues).
    pub issues: &'a [Issue],
    pub matched: bool,
    pub received_at: i64,
    pub sender_domain: &'a str,
    pub portal_hours: Option<f64>,
    /// `None` = the reader cannot tell yet.
    pub signed: Option<bool>,
    /// When the sheet this one replaces was received.
    pub replaces_received_at: Option<i64>,
    /// What the sheet itself states (as written), what the matched person's profile has, and whether the sender is a roster address.
    pub written_name: bool,
    pub written_code: bool,
    pub person_has_code: bool,
    pub sender_on_roster: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Correction,
    Question,
    Person,
    /// The sender is asked for details the sheet lacks.
    Missing,
    Note,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RuleIssue {
    pub code: &'static str,
    pub severity: Severity,
    /// Plain words for the sender. Empty for a `Person` issue or a `Note`.
    pub for_sender: String,
    pub for_manager: String,
}

/// Reader checks with their own rule above (or, for `person_unmatched`, their own severity).
const READER_CHECKS: [&str; 7] = ["unreadable", "no_hours", "total_mismatch", "impossible_hours", "model_days_dropped", "model_read", "person_unmatched"];

fn has_issue<'a>(r: &'a RecordView, code: &str) -> Option<&'a Issue> {
    r.issues.iter().find(|i| i.code == code)
}

fn hours(h: f64) -> String {
    if (h - h.round()).abs() < 1e-9 { format!("{}", h.round() as i64) } else { format!("{}", (h * 100.0).round() / 100.0) }
}

fn day_of(ms: i64, tz_min: i64) -> i64 {
    (ms + tz_min * 60_000).div_euclid(DAY_MS)
}

/// "2 Sep" for `YYYY-MM-DD`.
fn short_date(date: &str) -> String {
    match (date.get(5..7).and_then(|m| m.parse::<usize>().ok()), date.get(8..10).and_then(|d| d.parse::<u32>().ok())) {
        (Some(m @ 1..=12), Some(d)) => format!("{d} {}", MONTH_NAMES[m - 1]),
        _ => date.to_string(),
    }
}

fn full_month(month: &str) -> String {
    let name = month.get(5..7).and_then(|m| m.parse::<usize>().ok()).filter(|m| (1..=12).contains(m)).map(|m| MONTHS[m - 1].1);
    match (name, month.get(..4)) {
        (Some(n), Some(y)) => format!("{}{} {y}", n[..1].to_uppercase(), &n[1..]),
        _ => month.to_string(),
    }
}

/// "September 2026" for a whole calendar month, "5 Oct to 11 Oct 2026" or "11 Oct 2026" otherwise. Never ISO.
pub fn period_label(start: &str, end: &str) -> String {
    let (Some(a), Some(b)) = (clock::parse_date(start), clock::parse_date(end)) else { return format!("{start} to {end}") };
    let month = &start[..7.min(start.len())];
    if let Some((from, to)) = clock::month_bounds(month, 0)
        && a == from / DAY_MS
        && b == to / DAY_MS - 1
    {
        return full_month(month);
    }
    if a == b {
        return format!("{} {}", short_date(start), &start[..4]);
    }
    format!("{} to {} {}", short_date(start), short_date(end), &end[..4])
}

/// "1-30 Sep 2026" when both ends are in one month.
fn range_short(start: &str, end: &str) -> String {
    if start.get(..7) == end.get(..7) && start != end {
        let day = |d: &str| d[8..10].trim_start_matches('0').to_string();
        return format!("{}\u{2013}{} {} {}", day(start), day(end), MONTH_NAMES[start[5..7].parse::<usize>().unwrap_or(1).clamp(1, 12) - 1], &start[..4]);
    }
    period_label(start, end)
}

/// The dates (as days since 1970) a sheet received on `day` may cover, and the ones it is expected to cover.
/// Monthly: the month that just ended until `grace` days into the new month, then the new month, and a late sheet for
/// the month before that is still fine. Weekly works the same way with weeks; daily expects yesterday.
fn windows(cadence: Cadence, day: i64, grace: i64) -> ((i64, i64), (i64, i64)) {
    match cadence {
        Cadence::Monthly => {
            let month = clock::date_string(day)[..7].to_string();
            let range = |m: &str| clock::month_bounds(m, 0).map(|(a, b)| (a / DAY_MS, b / DAY_MS - 1)).unwrap_or((day, day));
            let first = range(&month).0;
            let current = range(&month);
            let previous = range(&clock::previous_month(&month));
            if day - first < grace { (previous, previous) } else { (current, (previous.0, current.1)) }
        }
        Cadence::Weekly => {
            let weekday = (day + 3).rem_euclid(7);
            let week_start = day - weekday;
            let last = (week_start - 7, week_start - 1);
            if weekday < grace { (last, last) } else { ((week_start, week_start + 6), (last.0, week_start + 6)) }
        }
        Cadence::Daily => ((day - 1, day - 1), (day - 1, day)),
    }
}

/// What to tell the sender: the expected period, and the one before it when a late sheet for it is fine too.
fn expected_label((from, to): (i64, i64), allowed: (i64, i64)) -> String {
    let label = |a: i64, b: i64| period_label(&clock::date_string(a), &clock::date_string(b));
    if allowed.0 < from { format!("{} or {}", label(allowed.0, from - 1), label(from, to)) } else { label(from, to) }
}

/// Hours per date. Rows without a date cannot be put on a day, so they are left out.
fn per_day(entries: &[Entry]) -> Vec<(String, f64)> {
    let mut days: Vec<(String, f64)> = vec![];
    for (e, date) in entries.iter().filter_map(|e| e.date.as_deref().map(|d| (e, d))) {
        match days.iter().position(|(x, _)| x == date) {
            Some(i) => days[i].1 += e.hours,
            None => days.push((date.to_string(), e.hours)),
        }
    }
    days
}

fn correction(code: &'static str, for_sender: String, for_manager: String) -> RuleIssue {
    RuleIssue { code, severity: Severity::Correction, for_sender, for_manager }
}

fn note(code: &'static str, for_manager: String) -> RuleIssue {
    RuleIssue { code, severity: Severity::Note, for_sender: String::new(), for_manager }
}

/// What the owner requires and neither the sheet nor the matched profile supplies: exactly these are asked of the sender, in a fixed order.
pub fn missing_fields(r: &RecordView, rules: &Rules) -> Vec<Field> {
    let want = |k: &str| rules.required.iter().any(|x| x == k);
    let unmatched = !r.matched;
    let mut out = vec![];
    if (want("name") || want("identity")) && unmatched && !r.written_name {
        out.push(Field::Name);
    }
    if ((want("identity") && unmatched) || want("employee_code")) && !r.written_code && !r.person_has_code {
        out.push(Field::EmployeeCode);
    }
    if want("email") && r.matched && !r.sender_on_roster {
        out.push(Field::Email);
    }
    if want("period") && has_issue(r, "period_assumed").is_some() {
        out.push(Field::Period);
    }
    out
}

pub fn check_against_rules(r: &RecordView, rules: &Rules, person: Option<&PersonRules>) -> Vec<RuleIssue> {
    let mut out = vec![];
    let unreadable = has_issue(r, "unreadable").is_some();
    let has_data = r.total.is_some() || !r.entries.is_empty();
    if unreadable {
        out.push(correction(
            "unreadable",
            "We could not read your timesheet file. Please send it again as an Excel file, a PDF, or typed into the email.".into(),
            "The timesheet file could not be read.".into(),
        ));
    } else if has_issue(r, "no_hours").is_some() {
        out.push(correction(
            "no_hours",
            "We found no hours in your timesheet. Please send it again with the hours for each day and the total.".into(),
            "No hours could be read from this timesheet.".into(),
        ));
    }
    for (code, tail) in [
        ("total_mismatch", "Please check it and send a corrected sheet."),
        ("impossible_hours", "Please correct it and send the sheet again."),
        ("model_days_dropped", "Please send it again with each date and its hours clearly written."),
    ] {
        if let Some(i) = has_issue(r, code) {
            out.push(correction(code, format!("{} {tail}", i.message.trim()), i.message.clone()));
        }
    }
    // Anything else the reader marked for a check is a problem with the sheet too: never an acknowledgement.
    if let Some(i) = has_issue(r, "model_read") {
        out.push(correction(
            "model_read",
            "We could not confirm that the days in your timesheet add up to its total. Please send it again with each date, its hours and the total clearly written.".into(),
            i.message.clone(),
        ));
    }
    for i in r.issues.iter().filter(|i| i.severity == CHECK && !READER_CHECKS.contains(&i.code.as_str())) {
        out.push(correction(
            "reader_check",
            format!("Something in your timesheet needs another look: {} Please check it and send a corrected sheet.", i.message.trim()),
            i.message.clone(),
        ));
    }
    if has_data && has_issue(r, "period_assumed").is_none() && !unreadable {
        let (cadence, day) = (rules.cadence, day_of(r.received_at, rules.tz_offset_minutes));
        let (expected, allowed) = windows(cadence, day, rules.grace_days);
        // Policy choice: a sheet is fine when its period overlaps the accepted dates at all. A period that starts in the
        // month before (4-4-5 calendars, a sheet from the 25th) is not called wrong; only a period wholly outside is.
        if let (Some(a), Some(b)) = (clock::parse_date(r.period.0), clock::parse_date(r.period.1))
            && (b < allowed.0 || a > allowed.1)
        {
            let (got, want) = (period_label(r.period.0, r.period.1), expected_label(expected, allowed));
            out.push(correction(
                "wrong_period",
                format!("Your timesheet is for {got}, but we expected {want}. Please send the one for {want}."),
                format!("The sheet is for {got}; {want} was expected."),
            ));
        }
    }
    if let Some(max) = rules.max_per_day {
        let mut over: Vec<String> = per_day(r.entries).into_iter().filter(|(_, h)| *h > max + 1e-9).map(|(d, _)| short_date(&d)).collect();
        over.sort();
        if !over.is_empty() {
            let days = over.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
            out.push(correction(
                "over_max_per_day",
                format!("The sheet has more than {} hours on {days}. Please check it and send a corrected sheet.", hours(max)),
                format!("More than {} hours a day on {days}.", hours(max)),
            ));
        }
    }
    if let (Some(total), Some(portal)) = (r.total, r.portal_hours)
        && (total - portal).abs() > rules.tolerance + 1e-9
    {
        out.push(RuleIssue {
            code: "portal_mismatch",
            severity: Severity::Question,
            for_sender: format!("Your sheet says {} h but the portal shows {} h for this period. Which one is right?", hours(total), hours(portal)),
            for_manager: format!("{} h on the sheet, {} h on the portal; you decide.", hours(total), hours(portal)),
        });
    }
    if rules.require_signature && r.signed == Some(false) {
        out.push(correction(
            "missing_signature",
            "Your timesheet needs your approver's signature. Please send it again with the signature.".into(),
            "The sheet is not signed.".into(),
        ));
    }
    if !r.matched {
        let who = has_issue(r, "person_unmatched").map_or_else(|| "The sender is not on your people list yet.".to_string(), |i| i.message.clone());
        out.push(RuleIssue { code: "person_unmatched", severity: Severity::Person, for_sender: String::new(), for_manager: who });
    }
    if let (Some(p), Some(total)) = (person, r.total)
        && let (Some(a), Some(b)) = (clock::parse_date(r.period.0), clock::parse_date(r.period.1))
        && b >= a
    {
        let weekdays = (a..=b).filter(|d| (d + 3).rem_euclid(7) < 5).count() as f64;
        let expected = match (p.weekly_target, p.daily_target) {
            (Some(w), _) => Some(w as f64 * ((b - a + 1) as f64 / 7.0)),
            (None, Some(d)) => Some(d as f64 * weekdays),
            _ => None,
        };
        if let Some(e) = expected.filter(|e| total + rules.tolerance < *e) {
            out.push(note("below_target", format!("{} h against an expected {} h for the period.", hours(total), hours((e * 10.0).round() / 10.0))));
        }
    }
    if !rules.client_domains.is_empty() && !r.sender_domain.is_empty() && !rules.client_domains.iter().any(|d| d.eq_ignore_ascii_case(r.sender_domain)) {
        out.push(note("outside_client_domains", format!("The sheet came from {}, which is not one of your client domains.", r.sender_domain)));
    }
    if let Some(earlier) = r.replaces_received_at {
        out.push(note("replaces_earlier", format!("This replaces the sheet received on {}.", short_date(&clock::date_string(day_of(earlier, rules.tz_offset_minutes))))));
    }
    out
}

/// One checked record, as `decision` needs it.
#[derive(Debug, Clone)]
pub struct Checked {
    pub record_id: String,
    pub person: Option<String>,
    pub period: (String, String),
    pub total: Option<f64>,
    pub issues: Vec<RuleIssue>,
    /// The name as written, when the sheet gives both a name and an employee ID: a complete identity that is not on the
    /// roster is an unknown person to be added, not a sheet with a missing field.
    pub identity: Option<String>,
    /// What this sheet lacks that the owner requires (`missing_fields`).
    pub missing: Vec<Field>,
}

fn who(c: &Checked) -> String {
    c.person.clone().unwrap_or_else(|| "Someone not yet matched to a person".into())
}

fn joined(values: impl Iterator<Item = String>) -> String {
    let mut out: Vec<String> = vec![];
    for v in values {
        if !out.contains(&v) {
            out.push(v);
        }
    }
    out.join("; ")
}

/// Clean sheet: acknowledge. Any correction: ask for a corrected sheet. Else any question: ask it. Only a person to
/// find: nothing to the sender, a wait for the link. Notes go to the manager and nowhere else.
pub fn decision(checked: &[Checked]) -> Decision {
    if checked.is_empty() {
        return Decision { outcome: Outcome::NothingToDo, message: None, manager_summary: None, waits: vec![], parked: None, asks: vec![] };
    }
    let with = |sev: Severity| -> Vec<&RuleIssue> { checked.iter().flat_map(|c| c.issues.iter()).filter(|i| i.severity == sev).collect() };
    let (corrections, questions, persons, notes) = (with(Severity::Correction), with(Severity::Question), with(Severity::Person), with(Severity::Note));
    let waits: Vec<Wait> = checked
        .iter()
        .filter(|c| c.issues.iter().any(|i| i.severity == Severity::Person))
        .map(|c| Wait::PersonLinked { record_id: c.record_id.clone() })
        .collect();
    let texts = |list: &[&RuleIssue]| -> Vec<IssueText> {
        let mut out: Vec<IssueText> = vec![];
        for i in list {
            if !out.iter().any(|t| t.text == i.for_sender) {
                out.push(IssueText { code: i.code.into(), text: i.for_sender.clone() });
            }
        }
        out
    };
    let note_lines: Vec<String> = notes.iter().map(|n| n.for_manager.clone()).collect();
    if !corrections.is_empty() {
        let all: Vec<&RuleIssue> = corrections.iter().chain(questions.iter()).copied().collect();
        let message = MessageRequest::template(MessageKind::Correction, vec![], texts(&all));
        return Decision { outcome: Outcome::Done, message: Some(message), manager_summary: None, waits, parked: None, asks: vec![] };
    }
    let asks: Vec<Field> = Field::ALL.into_iter().filter(|f| checked.iter().any(|c| c.missing.contains(f))).collect();
    if !asks.is_empty() {
        let message = ask_request(&asks, checked);
        return Decision { outcome: Outcome::Done, message: Some(message), manager_summary: None, waits, parked: None, asks };
    }
    if !questions.is_empty() {
        let message = MessageRequest::template(MessageKind::Question, vec![], texts(&questions));
        let headline = joined(checked.iter().filter(|c| c.issues.iter().any(|i| i.severity == Severity::Question)).map(|c| {
            let ask = c.issues.iter().filter(|i| i.severity == Severity::Question).map(|i| i.for_manager.clone()).collect::<Vec<_>>().join(" ");
            format!("{}'s timesheet for {}: {ask}", who(c), period_label(&c.period.0, &c.period.1))
        }));
        return Decision { outcome: Outcome::Done, message: Some(message), manager_summary: Some(ManagerSummary { headline, notes: note_lines }), waits, parked: None, asks: vec![] };
    }
    let fact = |label: &str, value: String| Fact { label: label.into(), value };
    if !persons.is_empty() {
        let why = joined(persons.iter().map(|p| p.for_manager.clone()));
        let unknown: Vec<&Checked> = checked.iter().filter(|c| c.issues.iter().any(|i| i.severity == Severity::Person)).collect();
        if unknown.iter().all(|c| c.identity.is_some()) {
            // A complete identity nobody has registered: the sender hears at once that it arrived (and nothing about hours), and the
            // acknowledgement that names the sheets waits for the person to be linked.
            let receipt = MessageRequest::template(MessageKind::Holding, vec![fact("About", "timesheet".into())], vec![]);
            let parked = ack_request(checked, |c| c.person.clone().or_else(|| c.identity.clone()).unwrap_or_default());
            return Decision { outcome: Outcome::NeedsPerson { why }, message: Some(receipt), manager_summary: None, waits, parked: Some(parked), asks: vec![] };
        }
        return Decision { outcome: Outcome::NeedsPerson { why }, message: None, manager_summary: None, waits, parked: None, asks: vec![] };
    }
    let headline = joined(checked.iter().map(|c| {
        let total = c.total.map_or_else(|| "no total".to_string(), |t| format!("{} h", hours(t)));
        format!("{}'s timesheet is recorded: {}, {total}", who(c), range_short(&c.period.0, &c.period.1))
    }));
    let message = ack_request(checked, who);
    Decision { outcome: Outcome::Done, message: Some(message), manager_summary: Some(ManagerSummary { headline, notes: note_lines }), waits, parked: None, asks: vec![] }
}

/// The question for missing details: one numbered line per missing item, the period when it is known (and not itself missing).
pub fn ask_request(asks: &[Field], checked: &[Checked]) -> MessageRequest {
    let mut facts: Vec<Fact> = asks.iter().map(|f| Fact { label: "Ask".into(), value: f.label().into() }).collect();
    if !asks.contains(&Field::Period) {
        let mut periods: Vec<String> = checked.iter().map(|c| period_label(&c.period.0, &c.period.1)).collect();
        periods.dedup();
        if let [one] = periods.as_slice() {
            facts.push(Fact { label: "Period".into(), value: one.clone() });
        }
    }
    let wanted = asks.iter().map(|f| f.label().to_lowercase()).collect::<Vec<_>>().join("; ");
    MessageRequest::template(MessageKind::Question, facts, vec![IssueText { code: MISSING_DATA.into(), text: format!("We need: {wanted}") }])
}

/// The acknowledgement of every sheet: one Name, Period, Hours run per sheet, so a reply can say which sheet is which.
fn ack_request(checked: &[Checked], name: impl Fn(&Checked) -> String) -> MessageRequest {
    let fact = |label: &str, value: String| Fact { label: label.into(), value };
    let mut rows: Vec<[Fact; 3]> = vec![];
    for c in checked {
        let row = [
            fact("Name", name(c)),
            fact("Period", period_label(&c.period.0, &c.period.1)),
            fact("Hours", c.total.map_or_else(|| "no total".to_string(), |t| format!("{} h", hours(t)))),
        ];
        if !rows.contains(&row) {
            rows.push(row);
        }
    }
    MessageRequest::template(MessageKind::Ack, rows.into_iter().flatten().collect(), vec![])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hr::team::clock::{DAY_MS, date_start_ms};

    const HOUR: i64 = 3_600_000;

    fn at(date: &str) -> i64 {
        date_start_ms(date, 0).unwrap() + 9 * HOUR
    }

    fn rules() -> Rules {
        Rules::from_settings(&TeamSettings::default())
    }

    fn entry(date: &str, hours: f64) -> Entry {
        Entry { date: Some(date.into()), hours, project: None, note: None }
    }

    struct Sheet {
        period: (&'static str, &'static str),
        total: Option<f64>,
        entries: Vec<Entry>,
        issues: Vec<Issue>,
        matched: bool,
        received: i64,
        portal: Option<f64>,
        signed: Option<bool>,
        domain: &'static str,
        replaces: Option<i64>,
        /// (name written, code written) on the sheet.
        written: (bool, bool),
        person_has_code: bool,
        on_roster: bool,
    }

    fn sept(received: &str) -> Sheet {
        Sheet {
            period: ("2026-09-01", "2026-09-30"),
            total: Some(168.0),
            entries: vec![entry("2026-09-01", 8.0), entry("2026-09-02", 8.0)],
            issues: vec![],
            matched: true,
            received: at(received),
            portal: None,
            signed: None,
            domain: "client.test",
            replaces: None,
            written: (true, true),
            person_has_code: true,
            on_roster: true,
        }
    }

    impl Sheet {
        fn check(&self, rules: &Rules, person: Option<&PersonRules>) -> Vec<RuleIssue> {
            check_against_rules(
                &RecordView {
                    period: self.period,
                    total: self.total,
                    entries: &self.entries,
                    issues: &self.issues,
                    matched: self.matched,
                    received_at: self.received,
                    sender_domain: self.domain,
                    portal_hours: self.portal,
                    signed: self.signed,
                    replaces_received_at: self.replaces,
                    written_name: self.written.0,
                    written_code: self.written.1,
                    person_has_code: self.person_has_code,
                    sender_on_roster: self.on_roster,
                },
                rules,
                person,
            )
        }
    }

    fn codes(issues: &[RuleIssue]) -> Vec<&'static str> {
        issues.iter().map(|i| i.code).collect()
    }

    fn checked(issues: Vec<RuleIssue>) -> Vec<Checked> {
        vec![Checked { record_id: "r1".into(), person: Some("Ravi Kumar".into()), period: ("2026-09-01".into(), "2026-09-30".into()), total: Some(168.0), issues, identity: None, missing: vec![] }]
    }

    fn issue(code: &'static str, severity: Severity, sender: &str) -> RuleIssue {
        RuleIssue { code, severity, for_sender: sender.into(), for_manager: format!("manager: {code}") }
    }

    #[test]
    fn clean_sheet_is_ack() {
        let s = sept("2026-10-02");
        let issues = s.check(&rules(), None);
        assert!(issues.is_empty(), "{issues:?}");
        let d = decision(&checked(issues));
        let m = d.message.expect("an acknowledgement is wanted");
        assert_eq!(m.kind, MessageKind::Ack);
        let facts: Vec<(&str, &str)> = m.facts.iter().map(|f| (f.label.as_str(), f.value.as_str())).collect();
        assert_eq!(facts, [("Name", "Ravi Kumar"), ("Period", "September 2026"), ("Hours", "168 h")]);
        assert_eq!(d.outcome, Outcome::Done);
        assert!(d.manager_summary.unwrap().headline.contains("Ravi Kumar"));
        assert!(d.waits.is_empty());
    }

    #[test]
    fn august_sheet_in_october_is_wrong_period() {
        let mut s = sept("2026-10-15");
        s.period = ("2026-08-01", "2026-08-31");
        let issues = s.check(&rules(), None);
        assert_eq!(codes(&issues), ["wrong_period"]);
        assert_eq!(issues[0].severity, Severity::Correction);
        assert!(issues[0].for_sender.contains("August 2026") && issues[0].for_sender.contains("September 2026"), "{}", issues[0].for_sender);
        assert!(!issues[0].for_sender.contains("2026-0"), "no ISO dates for the sender");
        let d = decision(&checked(issues));
        assert_eq!(d.message.unwrap().kind, MessageKind::Correction);
        assert!(d.manager_summary.is_none(), "a wrong month is not reported to the manager");
    }

    #[test]
    fn grace_day_boundary() {
        // Grace is 3 days. Up to the 3rd the month that just ended is expected; from the 4th the new month counts too.
        let mut october = sept("2026-10-03");
        october.period = ("2026-10-01", "2026-10-31");
        assert_eq!(codes(&october.check(&rules(), None)), ["wrong_period"], "3 Oct is still inside the grace days");
        october.received = at("2026-10-04");
        assert!(october.check(&rules(), None).is_empty(), "4 Oct expects October");
        let mut september_late = sept("2026-10-10");
        assert!(september_late.check(&rules(), None).is_empty(), "a late September sheet is still fine");
        september_late.received = at("2026-10-03");
        assert!(september_late.check(&rules(), None).is_empty());
        let mut august = sept("2026-10-03");
        august.period = ("2026-08-01", "2026-08-31");
        assert_eq!(codes(&august.check(&rules(), None)), ["wrong_period"]);
    }

    #[test]
    fn a_sheet_that_names_no_period_is_never_a_wrong_period() {
        let mut s = sept("2026-10-15");
        s.period = ("2026-10-01", "2026-10-31");
        s.issues.push(Issue { code: "period_assumed".into(), message: "x".into(), severity: "note" });
        assert!(s.check(&rules(), None).is_empty());
    }

    #[test]
    fn tolerance_is_hours_not_percent() {
        let mut s = sept("2026-10-02");
        s.portal = Some(168.4);
        assert!(s.check(&rules(), None).is_empty(), "0.4 h is inside a 0.5 h tolerance");
        s.portal = Some(169.0);
        assert_eq!(codes(&s.check(&rules(), None)), ["portal_mismatch"], "1 h is more than 0.5 h (0.5% of 168 would let it pass)");
        s.portal = Some(172.0);
        assert_eq!(codes(&s.check(&rules(), None)), ["portal_mismatch"], "0.5 h must not act as 5");
        let wide = Rules { tolerance: 5.0, ..rules() };
        assert!(s.check(&wide, None).is_empty());
    }

    #[test]
    fn portal_mismatch_is_question_with_both_figures() {
        let mut s = sept("2026-10-02");
        s.total = Some(176.0);
        s.portal = Some(168.0);
        let issues = s.check(&rules(), None);
        assert_eq!(codes(&issues), ["portal_mismatch"]);
        assert_eq!(issues[0].severity, Severity::Question);
        assert!(issues[0].for_sender.contains("176") && issues[0].for_sender.contains("168"), "{}", issues[0].for_sender);
        assert!(issues[0].for_manager.contains("176") && issues[0].for_manager.contains("168"));
        let d = decision(&checked(issues));
        assert_eq!(d.message.as_ref().unwrap().kind, MessageKind::Question);
        assert!(d.manager_summary.unwrap().headline.contains("176 h on the sheet, 168 h on the portal"));
    }

    #[test]
    fn unreadable_is_correction() {
        let mut s = sept("2026-10-02");
        s.total = None;
        s.entries.clear();
        s.issues.push(Issue { code: "unreadable".into(), message: "The file a.pdf could not be read.".into(), severity: "check" });
        let issues = s.check(&rules(), None);
        assert_eq!(codes(&issues), ["unreadable"]);
        assert_eq!(issues[0].severity, Severity::Correction);
        assert!(issues[0].for_sender.contains("Excel") && issues[0].for_sender.contains("PDF"), "{}", issues[0].for_sender);
        let mut none = sept("2026-10-02");
        none.issues.push(Issue { code: "no_hours".into(), message: "x".into(), severity: "check" });
        assert_eq!(codes(&none.check(&rules(), None)), ["no_hours"]);
    }

    #[test]
    fn reader_checks_become_corrections() {
        let mut s = sept("2026-10-02");
        for code in ["total_mismatch", "impossible_hours", "model_days_dropped"] {
            s.issues.push(Issue { code: code.into(), message: format!("{code} happened."), severity: "check" });
        }
        let issues = s.check(&rules(), None);
        assert_eq!(codes(&issues), ["total_mismatch", "impossible_hours", "model_days_dropped"]);
        assert!(issues.iter().all(|i| i.severity == Severity::Correction && !i.for_sender.is_empty()));
    }

    #[test]
    fn a_model_read_sheet_is_a_correction_not_an_ack() {
        let mut s = sept("2026-10-02");
        s.issues.push(Issue { code: "model_read".into(), message: "The days were read by an AI model.".into(), severity: "check" });
        let issues = s.check(&rules(), None);
        assert_eq!(codes(&issues), ["model_read"]);
        assert_eq!(issues[0].severity, Severity::Correction);
        assert_eq!(decision(&checked(issues)).message.unwrap().kind, MessageKind::Correction);
    }

    #[test]
    fn any_other_reader_check_is_a_correction_and_notes_are_not() {
        let mut s = sept("2026-10-02");
        s.issues.push(Issue { code: "something_new".into(), message: "Odd rows.".into(), severity: "check" });
        s.issues.push(Issue { code: "weekend_hours".into(), message: "Weekend.".into(), severity: "note" });
        let issues = s.check(&rules(), None);
        assert_eq!(codes(&issues), ["reader_check"]);
        assert!(issues[0].for_sender.contains("Odd rows."));
    }

    #[test]
    fn rows_without_a_date_never_add_up_to_a_day() {
        let mut s = sept("2026-10-02");
        s.entries = (0..5).map(|_| Entry { date: None, hours: 4.0, project: None, note: None }).collect();
        let limited = Rules { max_per_day: Some(10.0), ..rules() };
        assert!(s.check(&limited, None).is_empty(), "five undated rows of 4 h are not one 20 h day");
    }

    #[test]
    fn max_per_day() {
        let mut s = sept("2026-10-02");
        s.entries = vec![entry("2026-09-01", 8.0), entry("2026-09-02", 7.0), entry("2026-09-02", 4.0)];
        assert!(s.check(&rules(), None).is_empty(), "no limit set");
        let limited = Rules { max_per_day: Some(10.0), ..rules() };
        let issues = s.check(&limited, None);
        assert_eq!(codes(&issues), ["over_max_per_day"]);
        assert_eq!(issues[0].severity, Severity::Correction);
        assert!(issues[0].for_sender.contains("10") && issues[0].for_sender.contains("2 Sep"), "{}", issues[0].for_sender);
        let exact = Rules { max_per_day: Some(11.0), ..rules() };
        assert!(s.check(&exact, None).is_empty(), "exactly the limit is fine");
    }

    #[test]
    fn unmatched_waits_for_person() {
        let mut s = sept("2026-10-02");
        s.matched = false;
        let issues = s.check(&rules(), None);
        assert_eq!(codes(&issues), ["person_unmatched"]);
        assert_eq!(issues[0].severity, Severity::Person);
        assert!(issues[0].for_sender.is_empty());
        let mut c = checked(issues);
        c[0].person = None;
        let d = decision(&c);
        assert!(d.message.is_none(), "no identity written, so S9 will ask");
        assert!(matches!(d.outcome, Outcome::NeedsPerson { .. }));
        assert_eq!(d.waits, [Wait::PersonLinked { record_id: "r1".into() }]);
        assert!(d.manager_summary.is_none());
    }

    fn unknown(identity: Option<&str>) -> Vec<Checked> {
        let mut c = checked(vec![issue("person_unmatched", Severity::Person, "")]);
        c[0].person = None;
        c[0].identity = identity.map(str::to_string);
        c
    }

    #[test]
    fn a_complete_unknown_identity_gets_a_receipt_now_and_the_ack_parked() {
        let d = decision(&unknown(Some("Dinesh Rao")));
        let receipt = d.message.expect("the sender hears at once");
        assert_eq!(receipt.kind, MessageKind::Holding);
        assert_eq!(receipt.facts, [Fact { label: "About".into(), value: "timesheet".into() }]);
        assert!(receipt.issues.is_empty(), "no hours and no issue in a receipt");
        assert!(matches!(d.outcome, Outcome::NeedsPerson { .. }));
        assert_eq!(d.waits, [Wait::PersonLinked { record_id: "r1".into() }]);
        let parked = d.parked.expect("the ack waits for the link");
        assert_eq!(parked.kind, MessageKind::Ack);
        assert!(parked.facts.iter().any(|f| f.label == "Name" && f.value == "Dinesh Rao"), "{:?}", parked.facts);
        assert!(d.manager_summary.is_none());
    }

    #[test]
    fn an_identity_with_a_missing_field_gets_no_receipt_yet() {
        let d = decision(&unknown(None));
        assert!(d.message.is_none() && d.parked.is_none(), "asking for the missing field is S9's");
        assert!(matches!(d.outcome, Outcome::NeedsPerson { .. }));
    }

    #[test]
    fn a_correction_goes_now_even_when_the_person_is_unknown() {
        let mut c = unknown(Some("Dinesh Rao"));
        c[0].issues.push(issue("wrong_period", Severity::Correction, "Wrong month."));
        let d = decision(&c);
        assert_eq!(d.message.map(|m| m.kind), Some(MessageKind::Correction));
        assert!(d.parked.is_none(), "the corrected sheet is read again, so nothing is parked");
        assert_eq!(d.waits, [Wait::PersonLinked { record_id: "r1".into() }]);
    }

    fn required(keys: &[&str]) -> Rules {
        Rules { required: keys.iter().map(|k| (*k).to_string()).collect(), ..rules() }
    }

    #[test]
    fn a_sheet_without_a_code_asks_only_for_the_code() {
        let mut s = sept("2026-10-02");
        s.matched = false;
        s.person_has_code = false;
        s.written = (true, false);
        let r = RecordView { ..view(&s) };
        assert_eq!(missing_fields(&r, &required(&["identity"])), [Field::EmployeeCode]);
        assert_eq!(missing_fields(&r, &required(&["period"])), [], "what the owner does not require is never asked");
        s.written = (false, false);
        assert_eq!(missing_fields(&view(&s), &required(&["identity"])), [Field::Name, Field::EmployeeCode], "in a fixed order, both in one question");
        s.written = (true, true);
        assert_eq!(missing_fields(&view(&s), &required(&["identity"])), [], "a complete identity that nobody registered is a person to add, not a question");
    }

    #[test]
    fn the_profile_supplies_what_the_sheet_does_not() {
        let mut s = sept("2026-10-02");
        s.written = (true, false);
        s.person_has_code = true;
        assert_eq!(missing_fields(&view(&s), &required(&["employee_code"])), [], "the matched profile has the code");
        s.person_has_code = false;
        assert_eq!(missing_fields(&view(&s), &required(&["employee_code"])), [Field::EmployeeCode]);
        s.on_roster = false;
        assert_eq!(missing_fields(&view(&s), &required(&["email"])), [Field::Email], "the sender's address is not on the roster");
        s.issues.push(Issue { code: "period_assumed".into(), message: "x".into(), severity: "note" });
        assert_eq!(missing_fields(&view(&s), &required(&["period"])), [Field::Period]);
    }

    #[test]
    fn the_ask_names_each_missing_item_and_the_period_when_it_is_known() {
        let mut c = checked(vec![issue("person_unmatched", Severity::Person, "")]);
        c[0].missing = vec![Field::EmployeeCode];
        let d = decision(&c);
        let m = d.message.expect("an ask");
        assert!(m.is_ask() && m.kind == MessageKind::Question);
        assert_eq!(d.asks, [Field::EmployeeCode]);
        assert_eq!(m.facts.iter().filter(|f| f.label == "Ask").map(|f| f.value.as_str()).collect::<Vec<_>>(), ["Your employee ID"]);
        assert!(m.facts.iter().any(|f| f.label == "Period" && f.value == "September 2026"));
        // A correction goes first: the missing details are asked after the sheet is fixed.
        c[0].issues.push(issue("wrong_period", Severity::Correction, "Wrong month."));
        let d = decision(&c);
        assert_eq!((d.message.map(|m| m.kind), d.asks), (Some(MessageKind::Correction), vec![]));
    }

    fn view(s: &Sheet) -> RecordView<'_> {
        RecordView {
            period: s.period,
            total: s.total,
            entries: &s.entries,
            issues: &s.issues,
            matched: s.matched,
            received_at: s.received,
            sender_domain: s.domain,
            portal_hours: s.portal,
            signed: s.signed,
            replaces_received_at: s.replaces,
            written_name: s.written.0,
            written_code: s.written.1,
            person_has_code: s.person_has_code,
            sender_on_roster: s.on_roster,
        }
    }

    #[test]
    fn notes_never_message_the_sender() {
        let mut s = sept("2026-10-02");
        s.domain = "other.test";
        s.replaces = Some(at("2026-10-01") - DAY_MS);
        s.total = Some(100.0);
        let rules = Rules { client_domains: vec!["client.test".into()], ..rules() };
        let person = PersonRules { daily_target: Some(8), weekly_target: None };
        let issues = s.check(&rules, Some(&person));
        assert_eq!(codes(&issues), ["below_target", "outside_client_domains", "replaces_earlier"]);
        assert!(issues.iter().all(|i| i.severity == Severity::Note && i.for_sender.is_empty() && !i.for_manager.is_empty()));
        let d = decision(&checked(issues));
        let m = d.message.expect("still a clean sheet");
        assert_eq!(m.kind, MessageKind::Ack);
        assert!(m.issues.is_empty(), "no note reaches the sender");
        assert_eq!(d.manager_summary.unwrap().notes.len(), 3, "the manager does read the notes");
    }

    #[test]
    fn a_correction_hides_notes_from_the_sender_too() {
        let c = checked(vec![issue("wrong_period", Severity::Correction, "Wrong month."), issue("below_target", Severity::Note, "")]);
        let m = decision(&c).message.unwrap();
        assert_eq!(m.kind, MessageKind::Correction);
        assert_eq!(m.issues, [IssueText { code: "wrong_period".into(), text: "Wrong month.".into() }]);
    }

    #[test]
    fn a_correction_beats_a_question() {
        let c = checked(vec![issue("portal_mismatch", Severity::Question, "Which?"), issue("wrong_period", Severity::Correction, "Wrong month.")]);
        assert_eq!(decision(&c).message.unwrap().kind, MessageKind::Correction);
        assert_eq!(decision(&[]).outcome, Outcome::NothingToDo);
    }

    #[test]
    fn weekly_cadence_expects_last_week() {
        let weekly = Rules { cadence: Cadence::Weekly, ..rules() };
        // Monday 2026-10-12: last week is 5 to 11 Oct.
        let mut s = sept("2026-10-12");
        s.period = ("2026-10-05", "2026-10-11");
        assert!(s.check(&weekly, None).is_empty());
        s.period = ("2026-09-28", "2026-10-04");
        assert_eq!(codes(&s.check(&weekly, None)), ["wrong_period"], "the week before last");
        s.period = ("2026-10-12", "2026-10-18");
        assert_eq!(codes(&s.check(&weekly, None)), ["wrong_period"], "this week has not happened yet");
        s.received = at("2026-10-16");
        assert!(s.check(&weekly, None).is_empty(), "past the grace days the current week is expected");
    }

    #[test]
    fn missing_signature_needs_the_rule_and_a_reader_that_can_tell() {
        let mut s = sept("2026-10-02");
        s.signed = Some(false);
        assert!(s.check(&rules(), None).is_empty(), "off by default");
        let need = Rules { require_signature: true, ..rules() };
        assert_eq!(codes(&s.check(&need, None)), ["missing_signature"]);
        s.signed = None;
        assert!(s.check(&need, None).is_empty(), "the reader cannot see a signature yet, so nothing is asked");
    }
}
