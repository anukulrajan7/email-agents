use super::*;

#[derive(sqlx::FromRow)]
struct Pending {
    pub(super) id: String,
    title: String,
    body: String,
    message_id: Option<String>,
    escalation_level: i64,
    notified_json: String,
    person_id: Option<String>,
}

/// One alert in an email.
pub struct AlertLine {
    pub title: String,
    pub body: String,
    pub link: String,
    pub level: i64,
}

fn base_url(state: &AppState) -> String {
    state.config.app_base_url.trim_end_matches('/').to_string()
}

pub fn deep_link(base: &str, workspace_id: &str, message_id: Option<&str>) -> String {
    match message_id {
        Some(m) => format!("{base}/w/{workspace_id}/inbox/{m}"),
        None => format!("{base}/w/{workspace_id}/alerts"),
    }
}

async fn workspace_name(pool: &SqlitePool, workspace_id: &str) -> sqlx::Result<String> {
    Ok(sqlx::query_scalar("SELECT name FROM workspaces WHERE id = ?1")
        .bind(workspace_id)
        .fetch_optional(pool)
        .await?
        .unwrap_or_else(|| "your workspace".into()))
}

/// Sends every pending notification, batched into one email per recipient. Returns emails sent.
pub(super) async fn deliver(state: &AppState, workspace_id: &str, settings: &AlertSettings) -> sqlx::Result<usize> {
    let Some(mailer) = state.alert_mailer.clone() else {
        return Ok(0);
    };
    let pending: Vec<Pending> = sqlx::query_as(
        "SELECT id, title, body, message_id, escalation_level, notified_json, person_id FROM alerts
         WHERE workspace_id = ?1 AND status = 'open' AND notified_level < escalation_level ORDER BY created_at",
    )
    .bind(workspace_id)
    .fetch_all(&state.db)
    .await?;
    if pending.is_empty() {
        return Ok(0);
    }
    let base = base_url(state);
    let ws_name = workspace_name(&state.db, workspace_id).await?;
    let mut by_recipient: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, p) in pending.iter().enumerate() {
        let targets: Vec<String> = match &p.person_id {
            Some(person) => {
                crate::hr::team::escalation::recipients(&state.db, workspace_id, person, p.escalation_level, &settings.recipients).await?
            }
            None if p.escalation_level <= 1 || settings.escalate_to.is_empty() => settings.recipients.clone(),
            None => settings.escalate_to.clone(),
        };
        for t in targets {
            by_recipient.entry(t).or_default().push(i);
        }
    }
    let mut delivered: Vec<Vec<String>> = vec![vec![]; pending.len()];
    let mut sent = 0;
    for (to, idxs) in by_recipient {
        let lines: Vec<AlertLine> = idxs
            .iter()
            .map(|&i| AlertLine {
                title: pending[i].title.clone(),
                body: pending[i].body.clone(),
                link: deep_link(&base, workspace_id, pending[i].message_id.as_deref()),
                level: pending[i].escalation_level,
            })
            .collect();
        let (subject, text, html) = render_email(&ws_name, &base, workspace_id, &lines, None);
        let email = OutgoingEmail { kind: mail_kind::ALERT, to: &to, subject: &subject, text: &text, html: &html };
        match crate::platform::mailer::deliver(&state.db, mailer.as_ref(), email).await {
            Ok(()) => {
                sent += 1;
                for &i in &idxs {
                    delivered[i].push(to.clone());
                }
            }
            Err(e) => tracing::warn!(error = %e, "alert email failed; will retry next run"),
        }
    }
    for (p, to) in pending.iter().zip(delivered) {
        if to.is_empty() {
            continue;
        }
        let mut notified: Vec<String> = serde_json::from_str(&p.notified_json).unwrap_or_default();
        for t in to {
            if !notified.contains(&t) {
                notified.push(t);
            }
        }
        sqlx::query("UPDATE alerts SET notified_level = ?1, notified_json = ?2, updated_at = ?3 WHERE id = ?4")
            .bind(p.escalation_level)
            .bind(json!(notified).to_string())
            .bind(now_ms())
            .bind(&p.id)
            .execute(&state.db)
            .await?;
    }
    Ok(sent)
}

/// Claims today's digest (once per UTC day) and sends all open alerts to the recipients.
pub(super) async fn digest(state: &AppState, workspace_id: &str, settings: &AlertSettings, now: i64) -> sqlx::Result<bool> {
    let Some(mailer) = state.alert_mailer.clone() else {
        return Ok(false);
    };
    let day = now / 86_400_000;
    let claimed = sqlx::query(
        "INSERT INTO alert_settings (workspace_id, last_digest_day, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(workspace_id) DO UPDATE SET last_digest_day = excluded.last_digest_day
         WHERE alert_settings.last_digest_day IS NULL OR alert_settings.last_digest_day < excluded.last_digest_day",
    )
    .bind(workspace_id)
    .bind(day)
    .bind(now)
    .execute(&state.db)
    .await?
    .rows_affected()
        == 1;
    if !claimed {
        return Ok(false);
    }
    let open: Vec<(String, String, Option<String>, i64)> = sqlx::query_as(
        "SELECT title, body, message_id, escalation_level FROM alerts WHERE workspace_id = ?1 AND status = 'open'
         ORDER BY severity = 'high' DESC, created_at LIMIT 50",
    )
    .bind(workspace_id)
    .fetch_all(&state.db)
    .await?;
    if open.is_empty() {
        return Ok(false);
    }
    let base = base_url(state);
    let ws_name = workspace_name(&state.db, workspace_id).await?;
    let lines: Vec<AlertLine> = open
        .into_iter()
        .map(|(title, body, message_id, level)| AlertLine { title, body, link: deep_link(&base, workspace_id, message_id.as_deref()), level })
        .collect();
    let (subject, text, html) = render_email(&ws_name, &base, workspace_id, &lines, Some("Daily digest"));
    let mut any = false;
    for to in &settings.recipients {
        let email = OutgoingEmail { kind: mail_kind::ALERT_DIGEST, to, subject: &subject, text: &text, html: &html };
        match crate::platform::mailer::deliver(&state.db, mailer.as_ref(), email).await {
            Ok(()) => any = true,
            Err(e) => tracing::warn!(error = %e, "digest email failed"),
        }
    }
    Ok(any)
}

pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// Plain-text + simple HTML alert email. `heading` = `Some("Daily digest")` for the digest.
pub fn render_email(workspace_name: &str, base: &str, workspace_id: &str, lines: &[AlertLine], heading: Option<&str>) -> (String, String, String) {
    let title_of = |l: &AlertLine| if l.level > 1 { format!("Escalated: {}", l.title) } else { l.title.clone() };
    let subject = match (heading, lines) {
        (Some(h), _) => format!("[Post Room] {h}: {} open alert{}", lines.len(), if lines.len() == 1 { "" } else { "s" }),
        (None, [one]) => format!("[Post Room] {}", title_of(one)),
        (None, _) => format!("[Post Room] {} emails need attention", lines.len()),
    };
    let settings_link = format!("{base}/w/{workspace_id}/alerts");
    let intro = match heading {
        Some(_) => format!("Open alerts in {workspace_name}:"),
        None => format!("{} in {workspace_name} need{} attention:", if lines.len() == 1 { "One thing".to_string() } else { format!("{} things", lines.len()) }, if lines.len() == 1 { "s" } else { "" }),
    };
    let mut text = format!("Hi,\n\n{intro}\n\n");
    for l in lines {
        text.push_str(&format!("- {}\n  {}\n  Open: {}\n\n", title_of(l), l.body, l.link));
    }
    text.push_str(&format!("--\nPost Room alerts. All alerts and settings: {settings_link}\n"));

    let items: String = lines
        .iter()
        .map(|l| {
            format!(
                r#"<li style="margin:0 0 16px"><a href="{}" style="color:#34557A;font-weight:600">{}</a><br><span style="color:#5E5A50">{}</span></li>"#,
                escape_html(&l.link),
                escape_html(&title_of(l)),
                escape_html(&l.body)
            )
        })
        .collect();
    let html = format!(
        r#"<div style="font-family:Arial,sans-serif;background:#F3EFE6;padding:32px;color:#1C1B18">
<div style="height:6px;background:repeating-linear-gradient(135deg,#B73A26 0 10px,#F3EFE6 10px 20px,#34557A 20px 30px,#F3EFE6 30px 40px)"></div>
<h1 style="font-family:Georgia,serif;font-weight:600;font-size:22px;margin:24px 0 8px">{}</h1>
<ul style="padding-left:18px;margin:16px 0">{items}</ul>
<p style="font-size:12px;color:#5E5A50"><a href="{}" style="color:#5E5A50">All alerts and settings</a></p>
</div>"#,
        escape_html(&intro),
        escape_html(&settings_link)
    );
    (subject, text, html)
}
