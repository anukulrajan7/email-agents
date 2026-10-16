//! What a decision may wait for, and what wakes it. A wait is data on the decision; the thing it waits for happens
//! somewhere else (an admin links a person) and calls `plan::poke` with a `Wake`.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "on", rename_all = "snake_case")]
pub enum Wait {
    /// The sheet's author is not on the roster; the acknowledgement waits until someone links them.
    PersonLinked { record_id: String },
}

/// Something happened that a wait may be about.
#[derive(Debug, Clone, PartialEq)]
pub enum Wake {
    PersonLinked { record_ids: Vec<String> },
}

impl Wait {
    pub fn is_met_by(&self, wake: &Wake) -> bool {
        match (self, wake) {
            (Wait::PersonLinked { record_id }, Wake::PersonLinked { record_ids }) => record_ids.contains(record_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_meets_only_the_waits_for_its_own_sheets() {
        let wait = Wait::PersonLinked { record_id: "r1".into() };
        assert!(wait.is_met_by(&Wake::PersonLinked { record_ids: vec!["r0".into(), "r1".into()] }));
        assert!(!wait.is_met_by(&Wake::PersonLinked { record_ids: vec!["r2".into()] }));
    }
}
