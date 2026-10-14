//! Which learned preferences reach a draft: at most six active ones, best match first, within a character budget.

use sqlx::SqlitePool;

use super::preferences::{COLUMNS, Preference};
use crate::ai::embeddings::{Embedder, to_blob};

pub const MAX_PREFERENCES: usize = 6;
pub const MAX_PREFERENCE_CHARS_TOTAL: usize = 900;

fn from_blob(blob: &[u8]) -> Vec<f32> {
    blob.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect()
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Embeds `text` for storage on a preference.
pub async fn embed_one(embedder: &dyn Embedder, text: &str) -> Option<Vec<u8>> {
    let mut v = embedder.embed(&[text.to_string()]).await.ok()?;
    (v.len() == 1).then(|| to_blob(&v.remove(0)))
}

/// Active preferences for this agent and person (agent-less and team-wide ones included), ranked by similarity
/// to `query`, then evidence, then recency. Another agent's or person's preferences never match.
pub async fn for_draft(
    db: &SqlitePool,
    embedder: &dyn Embedder,
    ws: &str,
    agent_id: Option<&str>,
    owner_id: Option<&str>,
    query: &str,
) -> sqlx::Result<Vec<Preference>> {
    let mut prefs: Vec<Preference> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM agent_preferences
         WHERE workspace_id = ?1 AND status = 'active' AND (agent_id IS NULL OR agent_id = ?2) AND (user_id IS NULL OR user_id = ?3)
         ORDER BY evidence_count DESC, updated_at DESC LIMIT 60"
    ))
    .bind(ws)
    .bind(agent_id)
    .bind(owner_id)
    .fetch_all(db)
    .await?;
    if prefs.len() > MAX_PREFERENCES
        && let Ok(mut q) = embedder.embed(&[query.to_string()]).await
        && q.len() == 1
    {
        let q = q.remove(0);
        let score = |p: &Preference| p.embedding.as_deref().map(|b| dot(&from_blob(b), &q)).unwrap_or(f32::MIN);
        prefs.sort_by(|a, b| score(b).partial_cmp(&score(a)).unwrap_or(std::cmp::Ordering::Equal));
    }
    let mut budget = MAX_PREFERENCE_CHARS_TOTAL;
    let mut out = Vec::new();
    for p in prefs.into_iter().take(MAX_PREFERENCES) {
        let cost = p.content.chars().count();
        if cost > budget {
            break;
        }
        budget -= cost;
        out.push(p);
    }
    Ok(out)
}

/// One line per preference, for the drafter's system prompt.
pub fn lines(prefs: &[Preference]) -> Vec<String> {
    prefs.iter().map(|p| format!("({}) {}", p.category.replace('_', " "), p.content)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_roundtrip() {
        let v = vec![0.5f32, -1.0, 2.25];
        assert_eq!(from_blob(&to_blob(&v)), v);
    }
}
