//! Seeding: the lead, the desks each enabled pack brings, and one personal agent per member.
//! Every function is idempotent (a second call changes nothing) and never deletes an agent.

use sqlx::{SqliteConnection, SqlitePool};

use super::{
    spec::AgentSpec,
    store::{self, AgentRow, NewAgent},
    templates::{self, AgentTemplate},
    versions::{self, NewVersion},
};
use crate::packs;

pub fn personal_key(user_id: &str) -> String {
    format!("me_{user_id}")
}

fn template_spec(t: &AgentTemplate, key: &str) -> AgentSpec {
    let mut spec = t.spec();
    spec.key = key.to_string();
    spec
}

/// Creates the agent from its template unless it exists, with version 1. Returns the new id.
async fn ensure_template_agent(
    conn: &mut SqliteConnection,
    ws: &str,
    t: &AgentTemplate,
    key: &str,
    owner: Option<&str>,
    level: i64,
    name: &str,
) -> sqlx::Result<Option<String>> {
    let spec = template_spec(t, key);
    let created = store::insert(
        conn,
        NewAgent {
            workspace_id: ws,
            key,
            kind: t.kind,
            scope: t.scope,
            owner_user_id: owner,
            category: t.category,
            template_key: Some(t.key),
            template_version: Some(i64::from(t.version)),
            cloned_from: None,
            name,
            avatar: t.avatar,
            color: t.color,
            description: &spec.description,
            status: "active",
            autonomy_level: level,
            autonomy_cap: level.max(1),
            created_by: owner,
        },
    )
    .await?;
    if let Some(id) = &created {
        versions::create(conn, ws, id, &spec, NewVersion { source: "template", note: None, created_by: None, designer_session_id: None, activate: true }).await?;
    }
    Ok(created)
}

/// The lead agent (Meera). A workspace created before the registry already has the row from the
/// migration; it only needs its first version.
pub async fn ensure_lead(conn: &mut SqliteConnection, ws: &str) -> sqlx::Result<()> {
    let t = templates::lead_template();
    ensure_template_agent(conn, ws, t, "lead", None, t.initial_level, t.name).await?;
    Ok(())
}

/// The member's own agent. It starts at the lead's level, so a workspace that already lets routine
/// replies go keeps that behaviour for every mailbox.
pub async fn ensure_personal(conn: &mut SqliteConnection, ws: &str, user_id: &str) -> sqlx::Result<()> {
    ensure_lead(conn, ws).await?;
    let lead = store::lead(&mut *conn, ws).await?;
    let level = lead.as_ref().map_or(1, |l| l.autonomy_level.clamp(1, 2));
    let name = personal_name(lead.as_ref().map_or(templates::lead_template().name, |l| l.name.as_str()));
    ensure_template_agent(conn, ws, templates::personal_template(), &personal_key(user_id), Some(user_id), level, &name).await?;
    Ok(())
}

/// A member's own agent shows as "<lead name> (personal)", e.g. "Meera (personal)".
pub fn personal_name(lead_name: &str) -> String {
    format!("{} (personal)", lead_name.trim())
}

/// Renames personal agents still carrying the old generic name to "<lead name> (personal)".
async fn rename_generic_personal(conn: &mut SqliteConnection, ws: &str) -> sqlx::Result<()> {
    let Some(lead) = store::lead(&mut *conn, ws).await? else { return Ok(()) };
    sqlx::query("UPDATE agents SET name = ?2 WHERE workspace_id = ?1 AND scope = 'personal' AND name = ?3")
        .bind(ws)
        .bind(personal_name(&lead.name))
        .bind(templates::GENERIC_PERSONAL_NAME)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Seeds the desks these packs bring and restores the ones a disabled pack had paused.
pub async fn seed_for_packs(conn: &mut SqliteConnection, ws: &str, keys: &[&str]) -> sqlx::Result<()> {
    for t in templates::for_packs(keys) {
        let key = t.spec().key;
        ensure_template_agent(conn, ws, t, &key, None, t.initial_level, t.name).await?;
        if t.key == templates::REQUIREMENTS_KEY {
            adopt_requirement_cases(conn, ws).await?;
        }
        sqlx::query(
            "UPDATE agents SET status = 'active', paused_reason = NULL, updated_at = strftime('%s','now') * 1000
             WHERE workspace_id = ?1 AND template_key = ?2 AND status = 'paused' AND paused_reason = 'pack_off'",
        )
        .bind(ws)
        .bind(t.key)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Existing requirement cases belong to the Candidates & Requirements desk, not the lead.
async fn adopt_requirement_cases(conn: &mut SqliteConnection, ws: &str) -> sqlx::Result<()> {
    let (Some(lead), Some(desk)) = (store::lead(&mut *conn, ws).await?, store::by_template(&mut *conn, ws, templates::REQUIREMENTS_KEY).await?) else {
        return Ok(());
    };
    sqlx::query("UPDATE cases SET agent_id = ?1, agent_version = ?2 WHERE workspace_id = ?3 AND kind = 'requirement' AND agent_id = ?4")
        .bind(&desk.id)
        .bind(desk.active_version)
        .bind(ws)
        .bind(&lead.id)
        .execute(conn)
        .await?;
    Ok(())
}

/// A disabled pack pauses its desks. Nothing is deleted; enabling the pack again restores them.
pub async fn pause_for_packs_off(conn: &mut SqliteConnection, ws: &str, keys: &[&str]) -> sqlx::Result<()> {
    for t in templates::for_packs(keys) {
        sqlx::query(
            "UPDATE agents SET status = 'paused', paused_reason = 'pack_off', updated_at = strftime('%s','now') * 1000
             WHERE workspace_id = ?1 AND template_key = ?2 AND status = 'active'",
        )
        .bind(ws)
        .bind(t.key)
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "UPDATE drafts SET send_at = NULL
             WHERE workspace_id = ?1 AND send_at IS NOT NULL AND send_attempt_id IS NULL AND send_requested_by IS NULL AND status != 'sent'
               AND agent_id IN (SELECT id FROM agents WHERE workspace_id = ?1 AND template_key = ?2)",
        )
        .bind(ws)
        .bind(t.key)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Gives a first version to any agent that has none (agents the migration created).
async fn ensure_versions(conn: &mut SqliteConnection, ws: &str) -> sqlx::Result<()> {
    let bare = sqlx::query_as::<_, AgentRow>(&format!("SELECT {} FROM agents WHERE workspace_id = ?1 AND active_version IS NULL", store::AGENT_COLUMNS))
        .bind(ws)
        .fetch_all(&mut *conn)
        .await?;
    for agent in bare {
        let template = agent.template_key.as_deref().and_then(templates::template).unwrap_or_else(templates::lead_template);
        let spec = template_spec(template, &agent.key);
        versions::create(conn, ws, &agent.id, &spec, NewVersion { source: "backfill", note: None, created_by: None, designer_session_id: None, activate: true }).await?;
    }
    Ok(())
}

/// Untuned template agents follow the template: a newer template version becomes a new version.
/// Tuned agents keep their spec and show "update available" instead.
pub async fn upgrade_templates(conn: &mut SqliteConnection, ws: &str, catalogue: &[AgentTemplate]) -> sqlx::Result<u64> {
    let mut upgraded = 0;
    for t in catalogue {
        let outdated = sqlx::query_as::<_, AgentRow>(&format!(
            "SELECT {} FROM agents WHERE workspace_id = ?1 AND template_key = ?2 AND tuned = 0 AND COALESCE(template_version, 0) < ?3",
            store::AGENT_COLUMNS
        ))
        .bind(ws)
        .bind(t.key)
        .bind(i64::from(t.version))
        .fetch_all(&mut *conn)
        .await?;
        for agent in outdated {
            let spec = template_spec(t, &agent.key);
            let note = format!("Template updated to version {}", t.version);
            versions::create(conn, ws, &agent.id, &spec, NewVersion { source: "template", note: Some(&note), created_by: None, designer_session_id: None, activate: true }).await?;
            sqlx::query("UPDATE agents SET template_version = ?1 WHERE id = ?2").bind(i64::from(t.version)).bind(&agent.id).execute(&mut *conn).await?;
            upgraded += 1;
        }
    }
    Ok(upgraded)
}

/// Brings one workspace's registry up to date: lead, pack desks, a personal agent for every
/// member, first versions and template upgrades.
pub async fn seed_workspace(conn: &mut SqliteConnection, ws: &str) -> sqlx::Result<()> {
    ensure_lead(conn, ws).await?;
    let enabled = packs::enabled_keys(&mut *conn, ws).await?;
    seed_for_packs(conn, ws, &enabled).await?;
    let members: Vec<String> = sqlx::query_scalar("SELECT user_id FROM memberships WHERE workspace_id = ?1 ORDER BY rowid").bind(ws).fetch_all(&mut *conn).await?;
    for user_id in members {
        ensure_personal(conn, ws, &user_id).await?;
    }
    ensure_versions(conn, ws).await?;
    upgrade_templates(conn, ws, templates::TEMPLATES).await?;
    rename_generic_personal(conn, ws).await?;
    Ok(())
}

/// Read-only check: does this workspace still lack anything `seed_workspace` would add?
async fn incomplete(conn: &mut SqliteConnection, ws: &str) -> sqlx::Result<bool> {
    let gaps: i64 = sqlx::query_scalar(
        "SELECT (NOT EXISTS (SELECT 1 FROM agents WHERE workspace_id = ?1 AND kind = 'lead'))
             + EXISTS (SELECT 1 FROM agents WHERE workspace_id = ?1 AND active_version IS NULL)
             + EXISTS (SELECT 1 FROM agents WHERE workspace_id = ?1 AND scope = 'personal' AND name = 'My assistant')
             + EXISTS (SELECT 1 FROM memberships m WHERE m.workspace_id = ?1 AND NOT EXISTS
                         (SELECT 1 FROM agents a WHERE a.workspace_id = ?1 AND a.scope = 'personal' AND a.owner_user_id = m.user_id))",
    )
    .bind(ws)
    .fetch_one(&mut *conn)
    .await?;
    if gaps > 0 {
        return Ok(true);
    }
    let have: Vec<(String, Option<i64>, bool)> =
        sqlx::query_as("SELECT template_key, template_version, tuned FROM agents WHERE workspace_id = ?1 AND template_key IS NOT NULL")
            .bind(ws)
            .fetch_all(&mut *conn)
            .await?;
    let outdated = have.iter().any(|(key, version, tuned)| {
        !tuned && templates::template(key).is_some_and(|t| version.unwrap_or(0) < i64::from(t.version))
    });
    let enabled = packs::enabled_keys(&mut *conn, ws).await?;
    Ok(outdated || templates::for_packs(&enabled).any(|t| !have.iter().any(|(h, _, _)| h == t.key)))
}

/// Seeds the workspace only if something is missing (cheap and read-only when complete).
pub async fn ensure_workspace(db: &SqlitePool, ws: &str) -> sqlx::Result<()> {
    let mut conn = db.acquire().await?;
    if !incomplete(&mut conn, ws).await? {
        return Ok(());
    }
    drop(conn);
    let mut tx = db.begin().await?;
    seed_workspace(&mut tx, ws).await?;
    tx.commit().await
}

/// Startup backfill for every workspace (agents for existing packs and members).
pub async fn backfill_all(db: &SqlitePool) -> sqlx::Result<()> {
    let workspaces: Vec<String> = sqlx::query_scalar("SELECT id FROM workspaces").fetch_all(db).await?;
    for ws in workspaces {
        ensure_workspace(db, &ws).await?;
    }
    Ok(())
}
