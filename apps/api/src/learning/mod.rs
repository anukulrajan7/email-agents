//! The learning loop. Every user action that says "this was right" or "this was wrong" becomes an
//! append-only signal; edits and decline notes become suggested style preferences (kept apart from
//! facts); facts stay in `memories`. Learned items are filtered by code before they are stored and
//! never act as instructions.

pub mod context_pack;
pub mod edit_distance;
pub mod gain;
pub mod guard;
pub mod hooks;
pub mod preferences;
pub mod retrieve;
pub mod scope;
pub mod signals;
