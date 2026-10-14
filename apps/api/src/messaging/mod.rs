//! Messaging: Meera and the agents can send a new email, an in-app message or a notice to the owner, ask someone
//! and report the answer back, and run a support case with a vendor. Every send is audited, held for a few
//! seconds so it can be stopped, and told to the person ("I sent this to X") with a link.
//!
//! Who may send without a person: internal recipients, from an assistant that has earned level 2, with nothing
//! sensitive in it. Anything outside the workspace, anything about money, banks or security, attachments going
//! out, injection signals, and anything a person wrote themselves always waits for a person (see `rules`).

pub mod dispatch;
pub mod email;
pub mod inapp;
pub mod recipients;
pub mod rules;
pub mod support;
pub mod teams;
pub mod watch;

use serde_json::Value;

use crate::{
    assistant::{command::Citation, history},
    http::AppState,
};

/// Posts into a person's Meera conversation as Meera, with links under it.
pub async fn report(state: &AppState, ws: &str, user_id: &str, text: &str, citations: &[Citation], links: &Value) -> sqlx::Result<()> {
    history::append_with_links(&state.db, ws, user_id, "assistant", text, citations, &[], links).await
}

/// Same, inside the caller's transaction, so the report and the work it reports commit together.
pub async fn report_in(conn: &mut sqlx::SqliteConnection, ws: &str, user_id: &str, text: &str, citations: &[Citation], links: &Value) -> sqlx::Result<()> {
    history::append_with_links(conn, ws, user_id, "assistant", text, citations, &[], links).await
}
