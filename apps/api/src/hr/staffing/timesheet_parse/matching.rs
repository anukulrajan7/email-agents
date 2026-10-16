//! Matching a sheet's identity to a person of the directory: employee ID, then email, then the exact name.
//! Nothing here guesses: a similar or partial name is never a match (see `hr::team::identity::suggest`).

use std::collections::HashMap;

use super::Identity;
use crate::hr::staffing::name_key;

/// A person of the directory, as far as matching needs.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: String,
    pub name: String,
    pub code: Option<String>,
    pub emails: Vec<String>,
}

/// Links a person chose by hand: employee code (lower-case) or name key to person id.
#[derive(Debug, Clone, Default)]
pub struct Aliases {
    pub codes: HashMap<String, String>,
    pub names: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub person_id: Option<String>,
    /// code | alias | email | name | none
    pub method: &'static str,
    pub confidence: f64,
    pub ambiguous: bool,
}

impl Match {
    fn found(id: &str, method: &'static str, confidence: f64) -> Self {
        Self { person_id: Some(id.to_string()), method, confidence, ambiguous: false }
    }

    fn none(ambiguous: bool) -> Self {
        Self { person_id: None, method: "none", confidence: 0.0, ambiguous }
    }
}

fn tokens(name: &str) -> Vec<String> {
    name_key(name).split(' ').filter(|t| !t.is_empty()).map(str::to_string).collect()
}

/// The same name, whatever the order, case, punctuation or initials ("Ravi Kumar", "Kumar, Ravi"). A longer or shorter name
/// ("Ravi Kumar Singh", "Ravi") is another name.
pub fn names_agree(a: &str, b: &str) -> bool {
    let key = name_key(a);
    !key.is_empty() && key == name_key(b)
}

/// Who a timesheet is for. What the sheet says about the employee wins over who sent the mail, so a manager or the
/// HR mailbox forwarding a sheet never becomes its owner: the sender counts only when the name written is exactly theirs, or
/// when the sheet names nobody and was sent from the person's own mailbox (it was not forwarded).
pub fn match_person(people: &[Candidate], aliases: &Aliases, id: &Identity, sender: &str, original_sender: Option<&str>) -> Match {
    let by_email = |addr: &str| people.iter().find(|p| p.emails.iter().any(|e| e.eq_ignore_ascii_case(addr)));
    if let Some(code) = id.code.as_deref().map(str::to_lowercase) {
        if let Some(p) = people.iter().find(|p| p.code.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(&code))) {
            return Match::found(&p.id, "code", 0.99);
        }
        if let Some(p) = aliases.codes.get(&code).and_then(|pid| people.iter().find(|p| &p.id == pid)) {
            return Match::found(&p.id, "alias", 0.95);
        }
        // An ID is written but nobody has it: the sender or forwarder is not that person, so a person is asked.
        return match id.email.as_deref().and_then(by_email) {
            Some(p) if p.code.is_none() => Match::found(&p.id, "email", 0.95),
            _ => Match::none(false),
        };
    }
    if let Some(p) = id.email.as_deref().and_then(by_email) {
        return Match::found(&p.id, "email", 0.95);
    }
    let written = id.name.as_deref().filter(|n| !tokens(n).is_empty());
    for (addr, confidence) in original_sender.map(|a| (a, 0.9)).into_iter().chain(std::iter::once((sender, 0.88))) {
        if let Some(p) = by_email(addr)
            && match written {
                Some(w) => names_agree(&p.name, w),
                None => original_sender.is_none(),
            }
        {
            return Match::found(&p.id, "email", confidence);
        }
    }
    let Some(written) = written else { return Match::none(false) };
    if let Some(p) = aliases.names.get(&name_key(written)).and_then(|pid| people.iter().find(|p| &p.id == pid)) {
        return Match::found(&p.id, "alias", 0.95);
    }
    let key = name_key(written);
    let exact: Vec<&Candidate> = people.iter().filter(|p| name_key(&p.name) == key).collect();
    match exact.as_slice() {
        [p] => return Match::found(&p.id, "name", 0.85),
        [] => {}
        _ => return Match::none(true),
    }
    Match::none(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn people() -> Vec<Candidate> {
        let c = |id: &str, name: &str, code: Option<&str>, email: &str| Candidate { id: id.into(), name: name.into(), code: code.map(str::to_string), emails: vec![email.into()] };
        vec![
            c("p_ravi", "Ravi Kumar", Some("E1024"), "ravi@acme.test"),
            c("p_priya", "Priya Shah", Some("E2000"), "priya@acme.test"),
            c("p_dev", "Dev Patel", None, "dev@acme.test"),
            c("p_dev2", "Dev Patil", None, "devp@acme.test"),
        ]
    }

    fn who(name: Option<&str>, code: Option<&str>, email: Option<&str>) -> Identity {
        Identity { name: name.map(str::to_string), code: code.map(str::to_string), email: email.map(str::to_string), ..Default::default() }
    }

    #[test]
    fn matches_by_employee_code_first() {
        let m = match_person(&people(), &Aliases::default(), &who(Some("Someone Else"), Some("e1024"), None), "dev@acme.test", None);
        assert_eq!((m.person_id.as_deref(), m.method), (Some("p_ravi"), "code"));
    }

    #[test]
    fn matches_by_email_in_the_sheet_then_sender() {
        let m = match_person(&people(), &Aliases::default(), &who(None, None, Some("priya@acme.test")), "hr@acme.test", None);
        assert_eq!((m.person_id.as_deref(), m.method), (Some("p_priya"), "email"));
        let m = match_person(&people(), &Aliases::default(), &who(Some("Ravi Kumar"), None, None), "ravi@acme.test", None);
        assert_eq!((m.person_id.as_deref(), m.method), (Some("p_ravi"), "email"));
        let m = match_person(&people(), &Aliases::default(), &Identity::default(), "ravi@acme.test", None);
        assert_eq!(m.person_id.as_deref(), Some("p_ravi"));
    }

    #[test]
    fn matches_by_unique_name_only() {
        let m = match_person(&people(), &Aliases::default(), &who(Some("Kumar, Ravi"), None, None), "hr@acme.test", None);
        assert_eq!((m.person_id.as_deref(), m.method), (Some("p_ravi"), "name"));
        let m = match_person(&people(), &Aliases::default(), &who(Some("Ravi"), None, None), "hr@acme.test", None);
        assert_eq!(m.person_id, None, "one word is not enough");
        let m = match_person(&people(), &Aliases::default(), &who(Some("Dev Pat"), None, None), "hr@acme.test", None);
        assert_eq!(m.person_id, None);
        let m = match_person(&people(), &Aliases::default(), &who(Some("Nobody Known"), None, None), "hr@acme.test", None);
        assert_eq!((m.person_id, m.method), (None, "none"));
    }

    #[test]
    fn a_forwarded_sheet_belongs_to_the_employee_not_the_forwarder() {
        let m = match_person(&people(), &Aliases::default(), &who(Some("Ravi Kumar"), None, None), "priya@acme.test", None);
        assert_eq!((m.person_id.as_deref(), m.method), (Some("p_ravi"), "name"));
        let m = match_person(&people(), &Aliases::default(), &who(Some("Ravi Kumar"), Some("E1024"), None), "priya@acme.test", Some("hr@acme.test"));
        assert_eq!((m.person_id.as_deref(), m.method), (Some("p_ravi"), "code"));
        let m = match_person(&people(), &Aliases::default(), &who(Some("Unknown Person"), None, None), "priya@acme.test", None);
        assert_eq!(m.person_id, None, "the forwarder is not the owner of someone else's sheet");
    }

    #[test]
    fn a_remembered_link_matches_automatically() {
        let mut a = Aliases::default();
        a.codes.insert("x77".into(), "p_dev".into());
        a.names.insert(name_key("D. Pat"), "p_dev2".into());
        let m = match_person(&people(), &a, &who(None, Some("X77"), None), "hr@acme.test", None);
        assert_eq!((m.person_id.as_deref(), m.method), (Some("p_dev"), "alias"));
        let m = match_person(&people(), &a, &who(Some("D. Pat"), None, None), "hr@acme.test", None);
        assert_eq!((m.person_id.as_deref(), m.method), (Some("p_dev2"), "alias"));
    }

    #[test]
    fn the_senders_address_confirms_only_an_exactly_written_name() {
        let mut roster = people();
        roster.push(Candidate { id: "p_rahul".into(), name: "Rahul Verma".into(), code: None, emails: vec!["rahul@acme.test".into()] });
        roster.push(Candidate { id: "p_ravi_k".into(), name: "Ravi Kumar".into(), code: None, emails: vec!["ravi.k@acme.test".into()] });
        let none = Aliases::default();
        let from_rahul = match_person(&roster, &none, &who(Some("Rahul"), None, None), "rahul@acme.test", None);
        assert_eq!(from_rahul.person_id, None, "\"Rahul\" is not \"Rahul Verma\"");
        let singh = match_person(&roster, &none, &who(Some("Ravi Kumar Singh"), None, None), "ravi.k@acme.test", None);
        assert_eq!(singh.person_id, None, "a longer name is someone else, even from Ravi Kumar's mailbox");
        let exact = match_person(&roster, &none, &who(Some("Kumar, Ravi"), None, None), "ravi.k@acme.test", None);
        assert_eq!(exact.person_id.as_deref(), Some("p_ravi_k"));
    }

    #[test]
    fn a_nameless_sheet_belongs_to_the_sender_only_when_it_was_not_forwarded() {
        let none = Aliases::default();
        let own = match_person(&people(), &none, &Identity::default(), "ravi@acme.test", None);
        assert_eq!(own.person_id.as_deref(), Some("p_ravi"), "sent from their own mailbox");
        let forwarded = match_person(&people(), &none, &Identity::default(), "ravi@acme.test", Some("priya@acme.test"));
        assert_eq!(forwarded.person_id, None, "a forwarded nameless sheet is nobody's until a person says");
        let by_forwarder = match_person(&people(), &none, &Identity::default(), "hr@acme.test", Some("ravi@acme.test"));
        assert_eq!(by_forwarder.person_id, None);
    }

    #[test]
    fn a_name_that_contains_or_is_contained_in_another_is_never_a_match() {
        let mut roster = people();
        roster.push(Candidate { id: "p_rahul".into(), name: "Rahul Verma".into(), code: None, emails: vec!["rahul@acme.test".into()] });
        roster.push(Candidate { id: "p_ravi_s".into(), name: "Ravi Kumar Singh".into(), code: None, emails: vec!["ravis@acme.test".into()] });
        for written in ["Rahul Sharma", "Rahul", "Priya S.", "Ravi Kumar Singh Rao"] {
            let m = match_person(&roster, &Aliases::default(), &who(Some(written), None, None), "hr@acme.test", None);
            assert_eq!((m.person_id, m.method), (None, "none"), "{written}");
        }
        let only_singh: Vec<Candidate> = roster.iter().filter(|p| p.id == "p_ravi_s").cloned().collect();
        let m = match_person(&only_singh, &Aliases::default(), &who(Some("Ravi Kumar"), None, None), "hr@acme.test", None);
        assert_eq!((m.person_id, m.method), (None, "none"), "one roster name containing the written words used to link at 0.7");
        let m = match_person(&roster, &Aliases::default(), &who(Some("Ravi Kumar Singh"), None, None), "hr@acme.test", None);
        assert_eq!(m.person_id.as_deref(), Some("p_ravi_s"), "the whole name still matches");
    }
}
