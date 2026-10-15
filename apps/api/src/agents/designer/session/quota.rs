use super::*;

/// Takes one designer call slot before the model is asked. The checks and the insert are one statement, so
/// requests that race cannot go past the daily limit, the per-member limit, the session's budget, or run two
/// at once in one session. Returns the id of the reserved row.
pub(super) async fn reserve_call(conn: &mut sqlx::SqliteConnection, ws: &str, session: &Session, kind: &str) -> AppResult<String> {
    let id = new_id("dcl");
    let now = now_ms();
    let day_start = now - now.rem_euclid(DAY_MS);
    let inserted = sqlx::query(
        "INSERT INTO designer_calls (id, workspace_id, session_id, user_id, kind, status, cost_usd, created_at)
         SELECT ?1, ?2, ?3, ?4, ?5, 'pending', 0, ?6
         WHERE (?5 != 'design' OR (SELECT COUNT(*) FROM designer_calls WHERE workspace_id = ?2 AND kind = 'design' AND created_at >= ?7) < ?8)
           AND (?5 != 'personal' OR (SELECT COUNT(*) FROM designer_calls WHERE workspace_id = ?2 AND kind = 'personal' AND user_id = ?4 AND created_at >= ?7) < ?9)
           AND (SELECT cost_usd FROM designer_sessions WHERE id = ?3) < ?10
           AND NOT EXISTS (SELECT 1 FROM designer_calls WHERE session_id = ?3 AND status = 'pending' AND created_at > ?11 AND (kind = 'dry_run') = (?5 = 'dry_run'))
           AND (?5 != 'dry_run' OR (SELECT COUNT(*) FROM designer_calls WHERE session_id = ?3 AND kind = 'dry_run' AND created_at > ?12) < ?13)",
    )
    .bind(&id)
    .bind(ws)
    .bind(&session.id)
    .bind(&session.user_id)
    .bind(kind)
    .bind(now)
    .bind(day_start)
    .bind(DAILY_LIMIT)
    .bind(PERSONAL_DAILY_LIMIT)
    .bind(SESSION_BUDGET_USD)
    .bind(now - PENDING_STALE_MS)
    .bind(now - HOUR_MS)
    .bind(DRY_RUNS_PER_HOUR)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if inserted == 1 {
        return Ok(id);
    }
    let spent: f64 = sqlx::query_scalar("SELECT cost_usd FROM designer_sessions WHERE id = ?1").bind(&session.id).fetch_one(&mut *conn).await?;
    if spent >= SESSION_BUDGET_USD {
        return Err(AppError::LimitReached(format!("this session has reached its ${SESSION_BUDGET_USD:.2} limit; save what you have or start a new one")));
    }
    if kind == "dry_run" {
        let recent: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM designer_calls WHERE session_id = ?1 AND kind = 'dry_run' AND created_at > ?2")
            .bind(&session.id)
            .bind(now - HOUR_MS)
            .fetch_one(&mut *conn)
            .await?;
        if recent >= DRY_RUNS_PER_HOUR {
            return Err(AppError::LimitReached(format!("this session has run {DRY_RUNS_PER_HOUR} dry runs in the last hour; try again later")));
        }
    }
    if kind == "personal" {
        return Err(AppError::LimitReached(format!("you have used your {PERSONAL_DAILY_LIMIT} designer requests for your own agent today; try again tomorrow")));
    }
    if kind == "design" {
        let today: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM designer_calls WHERE workspace_id = ?1 AND kind = 'design' AND created_at >= ?2")
            .bind(ws)
            .bind(day_start)
            .fetch_one(&mut *conn)
            .await?;
        if today >= DAILY_LIMIT {
            return Err(AppError::LimitReached(format!("this workspace has used its {DAILY_LIMIT} designer requests for today; try again tomorrow")));
        }
    }
    Err(AppError::Conflict("the designer is still working on this session; wait for it to finish".into()))
}

/// Settles a reserved call with what it cost (failed calls are kept too).
pub(super) async fn settle_call(db: &SqlitePool, id: &str, status: &str, cost_usd: f64) -> sqlx::Result<()> {
    sqlx::query("UPDATE designer_calls SET status = ?1, cost_usd = ?2 WHERE id = ?3").bind(status).bind(cost_usd).bind(id).execute(db).await?;
    Ok(())
}
