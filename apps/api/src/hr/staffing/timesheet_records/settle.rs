use super::*;

/// A record already kept for the same person whose period overlaps.
#[derive(sqlx::FromRow)]
struct Kept {
    id: String,
    message_id: Option<String>,
    client: Option<String>,
    period_start: String,
    period_end: String,
    email_received_at: Option<i64>,
    issues_json: String,
}

impl Kept {
    fn assumed(&self) -> bool {
        serde_json::from_str::<Vec<Value>>(&self.issues_json).unwrap_or_default().iter().any(|i| i["code"] == "period_assumed")
    }

    fn received(&self) -> i64 {
        self.email_received_at.unwrap_or(0)
    }
}


fn client_key(c: Option<&str>) -> Option<String> {
    c.map(|c| c.trim().to_lowercase()).filter(|c| !c.is_empty())
}

fn same_mail(a: Option<&str>, b: Option<&str>) -> bool {
    a.is_some() && a == b
}

impl Placed<'_> {
    fn same_client(&self, old: &Kept) -> bool {
        client_key(self.client) == client_key(old.client.as_deref())
    }

    fn clients_differ(&self, old: &Kept) -> bool {
        matches!((client_key(self.client), client_key(old.client.as_deref())), (Some(a), Some(b)) if a != b)
    }

    /// The corrected version of `old`: same client, a stated period that covers it, from a later email.
    fn replaces(&self, old: &Kept) -> bool {
        !self.assumed
            && self.same_client(old)
            && self.start <= old.period_start.as_str()
            && self.end >= old.period_end.as_str()
            && !same_mail(self.message_id, old.message_id.as_deref())
            && self.received >= old.received()
    }

    /// `old` is a later email that would have replaced this one (mail read out of order).
    fn replaced_by(&self, old: &Kept) -> bool {
        !old.assumed()
            && self.same_client(old)
            && old.period_start.as_str() <= self.start
            && old.period_end.as_str() >= self.end
            && !same_mail(self.message_id, old.message_id.as_deref())
            && old.received() > self.received
    }
}


pub(super) fn duplicate_issue(start: &str, end: &str) -> Issue {
    note("possible_duplicate", format!("Another timesheet for this person covers {start} to {end}; check this is not a duplicate."))
}

/// Decides what an overlapping record means for the one being placed: replace it, be replaced, or sit beside it.
pub(super) async fn settle(conn: &mut SqliteConnection, ws: &str, person_id: &str, me: &Placed<'_>, now: i64) -> sqlx::Result<Settled> {
    let kept: Vec<Kept> = sqlx::query_as(
        "SELECT id, message_id, client, period_start, period_end, email_received_at, issues_json FROM timesheet_records
         WHERE workspace_id = ?1 AND person_id = ?2 AND status != 'replaced' AND id != ?5 AND period_start <= ?4 AND period_end >= ?3
         ORDER BY COALESCE(email_received_at, 0) DESC, created_at DESC, id",
    )
    .bind(ws)
    .bind(person_id)
    .bind(me.start)
    .bind(me.end)
    .bind(me.id)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Settled { superseded: kept.iter().any(|k| me.replaced_by(k)), ..Default::default() };
    for old in &kept {
        if !out.superseded && me.replaces(old) {
            sqlx::query("UPDATE timesheet_records SET status = 'replaced', updated_at = ?1 WHERE id = ?2 AND workspace_id = ?3").bind(now).bind(&old.id).bind(ws).execute(&mut *conn).await?;
            out.replaced.push(old.id.clone());
        } else if !out.superseded && !me.clients_differ(old) {
            if old.received() > me.received {
                add_issue(&mut *conn, ws, &old.id, duplicate_issue(me.start, me.end), now).await?;
            } else if out.duplicate_of.is_none() {
                out.duplicate_of = Some((old.period_start.clone(), old.period_end.clone()));
            }
        }
    }
    Ok(out)
}

async fn add_issue(conn: &mut SqliteConnection, ws: &str, id: &str, issue: Issue, now: i64) -> sqlx::Result<()> {
    let json: Option<String> = sqlx::query_scalar("SELECT issues_json FROM timesheet_records WHERE id = ?1 AND workspace_id = ?2").bind(id).bind(ws).fetch_optional(&mut *conn).await?;
    let Some(json) = json else { return Ok(()) };
    let mut issues: Vec<Value> = serde_json::from_str(&json).unwrap_or_default();
    if issues.iter().any(|i| i["code"] == issue.code) {
        return Ok(());
    }
    issues.push(json!(issue));
    sqlx::query("UPDATE timesheet_records SET issues_json = ?1, updated_at = ?2 WHERE id = ?3 AND workspace_id = ?4")
        .bind(serde_json::to_string(&issues).unwrap_or_else(|_| "[]".into()))
        .bind(now)
        .bind(id)
        .bind(ws)
        .execute(conn)
        .await?;
    Ok(())
}
