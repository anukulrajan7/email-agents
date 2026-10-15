//! Is learning working? Edit distance per agent over time: if people change drafts less than they used to,
//! the agent is improving. WP-D surfaces these numbers.

use serde::Serialize;
use sqlx::SqlitePool;

use crate::platform::db::iso;

const WEEK_MS: i64 = 7 * 86_400_000;
/// Signals that carry an edit distance for a draft a person finalised.
const DRAFT_KINDS: &str = "('draft_approved','draft_edited','approval_approved','approval_edited')";
/// Need at least this many measured drafts in each half before a gain is reported.
pub const MIN_DRAFTS_PER_HALF: usize = 3;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WeekPoint {
    pub week_start: String,
    pub drafts: usize,
    pub mean_edit_distance: f64,
    pub median_edit_distance: f64,
    /// Share of drafts a person sent without changing them.
    pub unchanged_rate: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LearningGain {
    pub drafts_measured: usize,
    /// Mean edit distance of the older half of the window.
    pub earlier_mean: Option<f64>,
    /// Mean edit distance of the newer half.
    pub recent_mean: Option<f64>,
    /// (earlier - recent) / earlier, as a percentage. Positive = people edit less than before. `None` until both halves have data.
    pub gain_pct: Option<f64>,
    pub active_preferences: i64,
    pub weekly: Vec<WeekPoint>,
}

fn mean(v: &[f64]) -> Option<f64> {
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}

fn median(sorted: &[f64]) -> f64 {
    match sorted.len() {
        0 => 0.0,
        n if n % 2 == 1 => sorted[n / 2],
        n => (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0,
    }
}

/// Pure part: `(created_at, edit_distance)` points, oldest first, over `weeks` weeks ending at `now`.
pub fn summarise(points: &[(i64, f64)], now: i64, weeks: i64, active_preferences: i64) -> LearningGain {
    let start = now - weeks * WEEK_MS;
    let mut weekly: Vec<WeekPoint> = Vec::new();
    for w in 0..weeks {
        let (from, to) = (start + w * WEEK_MS, start + (w + 1) * WEEK_MS);
        let mut d: Vec<f64> = points.iter().filter(|(t, _)| *t >= from && *t < to).map(|(_, d)| *d).collect();
        if d.is_empty() {
            continue;
        }
        d.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        weekly.push(WeekPoint {
            week_start: iso(from),
            drafts: d.len(),
            mean_edit_distance: mean(&d).unwrap_or(0.0),
            median_edit_distance: median(&d),
            unchanged_rate: d.iter().filter(|x| **x < super::edit_distance::TINY_EDIT).count() as f64 / d.len() as f64,
        });
    }
    let mid = start + (weeks * WEEK_MS) / 2;
    let inside = |t: &i64| *t >= start && *t <= now;
    let earlier: Vec<f64> = points.iter().filter(|(t, _)| inside(t) && *t < mid).map(|(_, d)| *d).collect();
    let recent: Vec<f64> = points.iter().filter(|(t, _)| inside(t) && *t >= mid).map(|(_, d)| *d).collect();
    let (earlier_mean, recent_mean) = (mean(&earlier), mean(&recent));
    let gain_pct = match (earlier_mean, recent_mean) {
        (Some(e), Some(r)) if e > 0.0 && earlier.len() >= MIN_DRAFTS_PER_HALF && recent.len() >= MIN_DRAFTS_PER_HALF => Some((e - r) / e * 100.0),
        _ => None,
    };
    LearningGain { drafts_measured: earlier.len() + recent.len(), earlier_mean, recent_mean, gain_pct, active_preferences, weekly }
}

/// Edit-distance trend and gain for one agent (or every agent when `agent_id` is `None`).
pub async fn for_agent(db: &SqlitePool, ws: &str, agent_id: Option<&str>, now: i64, weeks: i64) -> sqlx::Result<LearningGain> {
    let weeks = weeks.clamp(2, 52);
    let points: Vec<(i64, f64)> = sqlx::query_as(&format!(
        "SELECT created_at, edit_distance FROM feedback_events
         WHERE workspace_id = ?1 AND (?2 IS NULL OR agent_id = ?2) AND edit_distance IS NOT NULL AND kind IN {DRAFT_KINDS} AND created_at >= ?3
         ORDER BY created_at"
    ))
    .bind(ws)
    .bind(agent_id)
    .bind(now - weeks * WEEK_MS)
    .fetch_all(db)
    .await?;
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_preferences WHERE workspace_id = ?1 AND status = 'active' AND (?2 IS NULL OR agent_id = ?2)",
    )
    .bind(ws)
    .bind(agent_id)
    .fetch_one(db)
    .await?;
    Ok(summarise(&points, now, weeks, active))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_compares_the_newer_half_with_the_older() {
        let now = 8 * WEEK_MS;
        let early: Vec<(i64, f64)> = (0..4).map(|i| (i * WEEK_MS / 2 + 1, 0.4)).collect();
        let late: Vec<(i64, f64)> = (0..4).map(|i| (5 * WEEK_MS + i * WEEK_MS / 2, 0.2)).collect();
        let points: Vec<(i64, f64)> = early.into_iter().chain(late).collect();
        let g = summarise(&points, now, 8, 2);
        assert_eq!(g.drafts_measured, 8);
        assert!((g.gain_pct.unwrap() - 50.0).abs() < 1e-9, "{g:?}");
        assert!(g.weekly.len() >= 4);
    }

    #[test]
    fn no_gain_is_claimed_without_data_in_both_halves() {
        let g = summarise(&[(WEEK_MS * 7 + 5, 0.1)], 8 * WEEK_MS, 8, 0);
        assert_eq!(g.gain_pct, None);
    }
}
