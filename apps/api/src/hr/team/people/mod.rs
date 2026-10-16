//! The team directory: people, their emails, manager chain, joining date and targets.
//! Billable / non-billable and the client are HR & recruiting fields (ignored and rejected elsewhere).

mod import;
mod store;
mod validate;

use serde::Serialize;

pub use import::{ImportReport, RowError, import_csv};
pub use store::{get, insert, list, update};
pub use validate::{Context, Draft, FieldError, Fields, blank_draft, merge, parse_employee_type, validate};

pub const BILLABLE: &str = "billable";
pub const NON_BILLABLE: &str = "non_billable";
pub const MAX_CSV_ROWS: usize = 2_000;
const MAX_EMAILS: usize = 5;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Person {
    pub id: String,
    pub name: String,
    pub emails: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub employee_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    pub manager_id: Option<String>,
    pub role: Option<String>,
    pub employee_code: Option<String>,
    pub joining_date: Option<String>,
    pub daily_target: Option<i64>,
    pub weekly_target: Option<i64>,
    pub source_id: Option<String>,
    pub active: bool,
}

impl Person {
    /// Drops the HR & recruiting fields for workspaces without that pack.
    pub fn without_staffing(mut self) -> Self {
        self.employee_type = None;
        self.client = None;
        self
    }

    pub fn is_billable(&self) -> bool {
        self.employee_type.as_deref() == Some(BILLABLE)
    }

    pub fn primary_email(&self) -> Option<&str> {
        self.emails.first().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn employee_types_and_merge_rules() {
        assert_eq!(parse_employee_type(" Non-Billable "), Some(NON_BILLABLE));
        assert_eq!(parse_employee_type("BILLABLE"), Some(BILLABLE));
        assert_eq!(parse_employee_type("contractor"), None);
        let general = Context { staffing: false, hr_source_id: None, hr_connected: false };
        let f = Fields { name: Some(" Asha ".into()), emails: Some(vec!["Asha@Acme.com".into(), "asha@acme.com".into()]), ..Default::default() };
        let d = merge(blank_draft(), f, &general).unwrap();
        assert_eq!((d.name.as_str(), d.emails.clone()), ("Asha", vec!["asha@acme.com".to_string()]));
        let f = Fields { employee_type: Some("billable".into()), ..Default::default() };
        assert_eq!(merge(blank_draft(), f, &general).unwrap_err().field, "employee_type");
        let f = Fields { employee_type: Some("billable".into()), ..Default::default() };
        assert_eq!(merge(blank_draft(), f, &Context { staffing: true, hr_source_id: None, hr_connected: true }).unwrap().employee_type, BILLABLE);
    }
}
