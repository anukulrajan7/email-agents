//! Employee identity for people nobody registered. A timesheet always keeps the name exactly as written; the names
//! the roster does not know are grouped into `unresolved_identities` (one card per person, not one row per sheet);
//! and a group becomes a person only by an admin's action, written to `identity_changes` so it can be undone.
//!
//! - `resolve`: the pure rules (group key, who a name might be) and the admin's actions.
//! - `unresolved`: the grouped identities and what the admin sees.
//! - `changes`: the log of every identity change, and undo.
//!
//! Confidence here is evidence for a person to read, never a reason to link. Nothing in this module creates a user or
//! a membership: a sheet's author is not someone who can sign in.

pub mod changes;
pub mod resolve;
pub mod unresolved;

use crate::platform::{audit::Actor, error::AppError};

/// What an action on an identity can refuse for.
#[derive(Debug)]
pub enum IdentityError {
    NotFound,
    Conflict(String),
    Invalid(String),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for IdentityError {
    fn from(e: sqlx::Error) -> Self {
        IdentityError::Db(e)
    }
}

impl From<IdentityError> for AppError {
    fn from(e: IdentityError) -> Self {
        match e {
            IdentityError::NotFound => AppError::NotFound,
            IdentityError::Conflict(why) => AppError::Conflict(why),
            IdentityError::Invalid(why) => AppError::BadRequest(why),
            IdentityError::Db(e) => AppError::from(e),
        }
    }
}

/// `(actor_class, actor_id)` as `identity_changes` stores them.
pub(crate) fn actor_parts(actor: Actor<'_>) -> (&'static str, Option<&str>) {
    match actor {
        Actor::User(id) => ("user", Some(id)),
        Actor::System => ("system", None),
    }
}
