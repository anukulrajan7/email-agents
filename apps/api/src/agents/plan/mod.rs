//! Waiting and waking for expert decisions. An expert (the timesheet desk) can decide "tell the sender now, and the
//! rest when X happens". `poke` is X happening: it finds the messages whose parked requests were waiting for it and
//! lets the desk look at them again, which releases what is now ready. Everything it does is repeatable: looking again at
//! a message whose reply already went out files nothing new.

pub mod wait;

pub use wait::{Wait, Wake};

use crate::{
    hr::staffing::timesheet_records::desk,
    http::AppState,
    platform::flags::{self, Key},
};

/// Something a wait was about has happened. Returns how many messages were looked at again.
pub async fn poke(state: &AppState, ws: &str, wake: &Wake) -> sqlx::Result<usize> {
    if !flags::mode(&state.db, &state.config, ws, Key::Handoff).await?.acts() {
        return Ok(0);
    }
    let Wake::PersonLinked { record_ids } = wake;
    let messages: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT message_id FROM timesheet_records WHERE workspace_id = ?1 AND id IN (SELECT value FROM json_each(?2)) AND message_id IS NOT NULL ORDER BY message_id",
    )
    .bind(ws)
    .bind(serde_json::json!(record_ids).to_string())
    .fetch_all(&state.db)
    .await?;
    for message_id in &messages {
        desk::resume(state, ws, message_id).await?;
    }
    Ok(messages.len())
}
