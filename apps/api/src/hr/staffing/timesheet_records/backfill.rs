use super::*;

/// A mail the scan may try this many times: after that it is left for a person (or a new label) to bring back.
const MAX_ATTEMPTS: i64 = 3;

/// Records timesheets from mail labelled `timesheet` that has none yet (oldest first, a bounded batch per call).
/// Safe to repeat: mail that already has a record is skipped, and mail that gave nothing is tried at most
/// `MAX_ATTEMPTS` times so a model is not paid for the same mail every scan.
pub async fn backfill(state: &AppState, ws: &str, limit: i64) -> sqlx::Result<usize> {
    if !packs::enabled(&state.db, ws, packs::STAFFING).await? {
        return Ok(0);
    }
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT m.id FROM email_messages m JOIN classifications c ON c.message_id = m.id
         WHERE m.workspace_id = ?1 AND m.direction = 'inbound' AND m.suppressed = 0 AND c.category = ?2
           AND NOT EXISTS (SELECT 1 FROM timesheet_records r WHERE r.message_id = m.id)
           AND COALESCE((SELECT a.attempts FROM timesheet_attempts a WHERE a.message_id = m.id), 0) < ?4
         ORDER BY m.received_at, m.id LIMIT ?3",
    )
    .bind(ws)
    .bind(LABEL)
    .bind(limit)
    .bind(MAX_ATTEMPTS)
    .fetch_all(&state.db)
    .await?;
    let mut recorded = 0;
    for id in ids {
        let run = record_message(state, ws, &id).await?;
        let made = run.records.iter().filter(|r| r.created).count();
        recorded += made;
        if made == 0 && !run.waiting_for_files {
            sqlx::query(
                "INSERT INTO timesheet_attempts (message_id, workspace_id, attempts, last_at) VALUES (?1, ?2, 1, ?3)
                 ON CONFLICT(message_id) DO UPDATE SET attempts = attempts + 1, last_at = excluded.last_at",
            )
            .bind(&id)
            .bind(ws)
            .bind(now_ms())
            .execute(&state.db)
            .await?;
        }
    }
    Ok(recorded)
}

/// Forgetting a sender erases what was kept of their timesheets: the records and the code / name links a person
/// made for them. (Deleting an email or disconnecting a mailbox does not call this: the records are kept on purpose.)
pub async fn erase_sender(conn: &mut SqliteConnection, ws: &str, sender: &str) -> sqlx::Result<u64> {
    let address = address_of(sender);
    let who: Vec<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT employee_code, person_name FROM timesheet_records WHERE workspace_id = ?1 AND lower(sender_address) = lower(?2)").bind(ws).bind(&address).fetch_all(&mut *conn).await?;
    for (code, name) in who {
        if let Some(code) = code.map(|c| c.to_lowercase()) {
            sqlx::query("DELETE FROM timesheet_aliases WHERE workspace_id = ?1 AND kind = 'code' AND value = ?2").bind(ws).bind(code).execute(&mut *conn).await?;
        }
        if let Some(key) = name.as_deref().map(name_key).filter(|k| !k.is_empty()) {
            sqlx::query("DELETE FROM timesheet_aliases WHERE workspace_id = ?1 AND kind = 'name' AND value = ?2").bind(ws).bind(key).execute(&mut *conn).await?;
        }
    }
    let erased = sqlx::query("DELETE FROM timesheet_records WHERE workspace_id = ?1 AND lower(sender_address) = lower(?2)").bind(ws).bind(&address).execute(&mut *conn).await?.rows_affected();
    // The unknown names they brought go too, unless another sender's sheets still stand under the card (then only the address goes).
    sqlx::query("DELETE FROM unresolved_identities WHERE workspace_id = ?1 AND NOT EXISTS (SELECT 1 FROM timesheet_records r WHERE r.identity_id = unresolved_identities.id)").bind(ws).execute(&mut *conn).await?;
    sqlx::query("UPDATE unresolved_identities SET sender_address = NULL WHERE workspace_id = ?1 AND lower(sender_address) = lower(?2)").bind(ws).bind(&address).execute(conn).await?;
    Ok(erased)
}
