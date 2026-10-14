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

/// The member's own agent. It starts acting on its own (level 3, behind the hold and the fixed
/// sensitive-case floor). Its name is Meera for everyone.
pub async fn ensure_personal(conn: &mut SqliteConnection, ws: &str, user_id: &str) -> sqlx::Result<()> {
    ensure_lead(conn, ws).await?;
    let t = templates::personal_template();
    ensure_template_agent(conn, ws, t, &personal_key(user_id), Some(user_id), t.initial_level, templates::PERSONAL_NAME).await?;
    Ok(())
}

/// The lead's first personality line, before the lead was called Aarav.
const OLD_LEAD_PERSONALITY: &str = "I'm Meera, the team's email assistant. I'm warm, upbeat and a little playful, \
but always professional: friendly greetings, clear next steps, no jargon, and never more words than needed.";

/// The lead's personality line while it was called Aarav.
const AARAV_PERSONALITY: &str = "I'm Aarav, the team's email assistant. I'm calm and organised, \
and always professional: friendly greetings, clear next steps, no jargon, and never more words than needed.";

/// Gives agents that still carry an old default name (and were never tuned) their new one. A name an
/// admin chose, or an agent that was tuned, is left alone.
async fn rename_old_defaults(conn: &mut SqliteConnection, ws: &str) -> sqlx::Result<()> {
    for (key, olds) in templates::OLD_DEFAULT_NAMES {
        let Some(t) = templates::template(key) else { continue };
        for old in *olds {
            let renamed = sqlx::query("UPDATE agents SET name = ?1 WHERE workspace_id = ?2 AND template_key = ?3 AND tuned = 0 AND name = ?4")
                .bind(t.name)
                .bind(ws)
                .bind(t.key)
                .bind(old)
                .execute(&mut *conn)
                .await?
                .rows_affected();
            if renamed > 0 && t.key == templates::LEAD_KEY {
                sqlx::query("UPDATE workspace_agents SET name = ?1, persona = REPLACE(REPLACE(persona, ?2, ?3), ?4, ?3) WHERE workspace_id = ?5 AND name = ?6")
                    .bind(crate::agent_profiles::DEFAULT_NAME)
                    .bind(OLD_LEAD_PERSONALITY)
                    .bind(crate::agent_profiles::DEFAULT_PERSONALITY)
                    .bind(AARAV_PERSONALITY)
                    .bind(ws)
                    .bind(old)
                    .execute(&mut *conn)
                    .await?;
            }
        }
    }
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

/// The jobs of the agents that were merged into an expert belong to that expert now. An untuned merged
/// agent is archived and its open cases, pending approvals, unsent drafts and routes move to the expert
/// (the lead for the inbox assistant); one an admin tuned stays active as a custom agent. Idempotent.
pub async fn consolidate_roster(conn: &mut SqliteConnection, ws: &str) -> sqlx::Result<()> {
    for retired in templates::RETIRED {
        let Some(target_key) = templates::merged_into(retired.key) else { continue };
        let agents = sqlx::query_as::<_, AgentRow>(&format!(
            "SELECT {} FROM agents WHERE workspace_id = ?1 AND template_key = ?2 AND kind = 'system' AND status != 'archived'",
            store::AGENT_COLUMNS
        ))
        .bind(ws)
        .bind(retired.key)
        .fetch_all(&mut *conn)
        .await?;
        for agent in agents {
            if agent.tuned {
                sqlx::query("UPDATE agents SET kind = 'custom', template_key = NULL, template_version = NULL, updated_at = ?1 WHERE id = ?2")
                    .bind(crate::db::now_ms())
                    .bind(&agent.id)
                    .execute(&mut *conn)
                    .await?;
                continue;
            }
            let target = match store::by_template(&mut *conn, ws, target_key).await? {
                Some(t) => Some(t),
                None => store::lead(&mut *conn, ws).await?,
            };
            let Some(target) = target else {
                tracing::warn!(workspace = ws, agent = %agent.id, "no agent to take over from a merged desk; leaving it active");
                continue;
            };
            repoint_work(conn, ws, &agent.id, &target).await?;
            store::set_status(conn, ws, &agent.id, "archived", None).await?;
        }
    }
    Ok(())
}

async fn repoint_work(conn: &mut SqliteConnection, ws: &str, from: &str, to: &AgentRow) -> sqlx::Result<()> {
    let version = to.active_version.unwrap_or(1);
    for sql in [
        "UPDATE cases SET agent_id = ?1, agent_version = ?2 WHERE workspace_id = ?3 AND agent_id = ?4 AND shadow = 0 AND status NOT IN ('done', 'failed', 'cancelled')",
        "UPDATE agent_approvals SET agent_id = ?1 WHERE workspace_id = ?3 AND agent_id = ?4 AND status = 'pending' AND ?2 IS NOT NULL",
        "UPDATE drafts SET agent_id = ?1 WHERE workspace_id = ?3 AND agent_id = ?4 AND status != 'sent' AND ?2 IS NOT NULL",
        "UPDATE message_routes SET agent_id = ?1, agent_version = ?2 WHERE workspace_id = ?3 AND agent_id = ?4",
    ] {
        sqlx::query(sql).bind(&to.id).bind(version).bind(ws).bind(from).execute(&mut *conn).await?;
    }
    Ok(())
}

/// Active, untuned helpers that should be acting on their own but are not: Meera (lead and personal) and the
/// experts seeded at level 3, still at their seeded cap, in a workspace whose send setting is "auto".
/// Anyone a person touched (a user-actor level or limit change) or an incident demoted is left alone.
async fn automatic_candidates(conn: &mut SqliteConnection, ws: &str) -> sqlx::Result<Vec<(String, i64)>> {
    let keys: Vec<&str> = templates::TEMPLATES.iter().filter(|t| t.initial_level >= 3).map(|t| t.key).collect();
    sqlx::query_as(
        "SELECT a.id, a.autonomy_level FROM agents a
         WHERE a.workspace_id = ?1 AND a.status = 'active' AND a.tuned = 0 AND a.autonomy_level < 3 AND a.autonomy_cap BETWEEN 1 AND 2
           AND EXISTS (SELECT 1 FROM workspace_agents w WHERE w.workspace_id = a.workspace_id AND json_valid(w.permissions_json)
                       AND json_extract(w.permissions_json, '$.send') = 'auto')
           AND (a.kind = 'lead' OR a.scope = 'personal' OR a.template_key IN (SELECT value FROM json_each(?2)))
           AND NOT EXISTS (SELECT 1 FROM autonomy_changes c WHERE c.agent_id = a.id AND (c.actor LIKE 'user:%' OR (c.to_level < c.from_level AND c.actor = 'system')))",
    )
    .bind(ws)
    .bind(serde_json::json!(keys).to_string())
    .fetch_all(conn)
    .await
}

/// Meera and the field experts act on their own by default. A workspace whose admin never saved the
/// agent's settings moves from "ask" to "send automatically". Whoever chose "auto", the untuned helpers
/// still at their seeded limit then move to level 3 (behind the hold and the sensitive-case floor), each
/// change recorded. A workspace whose admin chose "ask" is left alone, and so is any helper a person
/// limited or an incident demoted. Idempotent.
pub async fn adopt_automatic_defaults(conn: &mut SqliteConnection, ws: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE workspace_agents SET permissions_json = json_set(permissions_json, '$.send', 'auto')
         WHERE workspace_id = ?1 AND updated_by IS NULL AND json_valid(permissions_json) AND json_extract(permissions_json, '$.send') = 'ask'",
    )
    .bind(ws)
    .execute(&mut *conn)
    .await?;
    for (id, level) in automatic_candidates(conn, ws).await? {
        store::set_autonomy_cap(conn, ws, &id, 3).await?;
        if store::set_autonomy_level(conn, ws, &id, 3).await? {
            crate::evals::gate::record_change(conn, ws, &id, level, 3, "Automatic by default", serde_json::json!({}), "system").await?;
        }
    }
    Ok(())
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
    consolidate_roster(conn, ws).await?;
    rename_old_defaults(conn, ws).await?;
    adopt_automatic_defaults(conn, ws).await?;
    Ok(())
}

/// Read-only check: does this workspace still lack anything `seed_workspace` would add?
async fn incomplete(conn: &mut SqliteConnection, ws: &str) -> sqlx::Result<bool> {
    let gaps: i64 = sqlx::query_scalar(
        "SELECT (NOT EXISTS (SELECT 1 FROM agents WHERE workspace_id = ?1 AND kind = 'lead'))
             + EXISTS (SELECT 1 FROM agents WHERE workspace_id = ?1 AND active_version IS NULL)
             + EXISTS (SELECT 1 FROM memberships m WHERE m.workspace_id = ?1 AND NOT EXISTS
                         (SELECT 1 FROM agents a WHERE a.workspace_id = ?1 AND a.scope = 'personal' AND a.owner_user_id = m.user_id))",
    )
    .bind(ws)
    .fetch_one(&mut *conn)
    .await?;
    if gaps > 0 {
        return Ok(true);
    }
    let retired: Vec<&str> = templates::RETIRED.iter().map(|t| t.key).collect();
    let pending: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM agents WHERE workspace_id = ?1 AND status != 'archived' AND kind = 'system' AND template_key IN (SELECT value FROM json_each(?2)))
             OR EXISTS (SELECT 1 FROM workspace_agents WHERE workspace_id = ?1 AND updated_by IS NULL AND json_valid(permissions_json)
                        AND json_extract(permissions_json, '$.send') = 'ask')",
    )
    .bind(ws)
    .bind(serde_json::json!(retired).to_string())
    .fetch_one(&mut *conn)
    .await?;
    if pending || !automatic_candidates(conn, ws).await?.is_empty() {
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
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
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
