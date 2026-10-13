use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::AppState;
use crate::{
    auth::{AuthUser, require_role},
    db::{iso, now_ms},
    domain::{OTHER_LABEL, Role},
    error::{AppError, AppResult},
};

const DAY_MS: i64 = 86_400_000;
const DEFAULT_DAYS: i64 = 30;
const MAX_DAYS: i64 = 180;
const ADD_LABEL_SHARE: f64 = 0.25;
const UNRELIABLE_RATE: f64 = 0.2;
const UNRELIABLE_MIN_EMAILS: i64 = 5;

#[derive(Deserialize)]
pub struct HealthQuery {
    days: Option<i64>,
}

#[derive(Serialize)]
struct TrendPoint {
    date: String,
    count: i64,
}

#[derive(Serialize)]
struct LabelHealth {
    key: String,
    name: String,
    count: i64,
    share: f64,
    auto: i64,
    corrected_away: i64,
    corrected_into: i64,
    correction_rate: f64,
    avg_confidence: Option<f64>,
    trend: Vec<TrendPoint>,
}

#[derive(Serialize)]
struct Advice {
    kind: &'static str,
    label_key: Option<String>,
    text: String,
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn ratio(n: i64, d: i64) -> f64 {
    if d == 0 { 0.0 } else { round2(n as f64 / d as f64) }
}

/// Operator: how well automatic labelling is working over the last `days` days of inbound mail.
pub async fn health(
    State(state): State<AppState>,
    user: AuthUser,
    Path(ws): Path<String>,
    Query(q): Query<HealthQuery>,
) -> AppResult<Json<Value>> {
    require_role(&state.db, &ws, &user, Role::Operator).await?;
    let days = q.days.unwrap_or(DEFAULT_DAYS);
    if !(1..=MAX_DAYS).contains(&days) {
        return Err(AppError::BadRequest(format!("days must be between 1 and {MAX_DAYS}")));
    }
    let today = now_ms() / DAY_MS;
    let first_day = today - (days - 1);
    let since = first_day * DAY_MS;

    let labels: Vec<(String, String, bool)> = sqlx::query_as(
        "SELECT key, name, archived FROM labels WHERE workspace_id = ?1 ORDER BY archived, position, created_at",
    )
    .bind(&ws)
    .fetch_all(&state.db)
    .await?;

    // Per current label: emails, how many a person moved in, mean confidence of the AI's own picks.
    let stats: Vec<(String, i64, i64, Option<f64>)> = sqlx::query_as(
        "SELECT c.category, COUNT(*),
                CAST(COALESCE(SUM(c.source = 'manual' AND (c.ai_category IS NULL OR c.ai_category != c.category)), 0) AS INTEGER),
                AVG(CASE WHEN c.source != 'manual' THEN c.confidence END)
         FROM classifications c JOIN email_messages m ON m.id = c.message_id
         WHERE m.workspace_id = ?1 AND m.direction = 'inbound' AND m.received_at >= ?2
         GROUP BY c.category",
    )
    .bind(&ws)
    .bind(since)
    .fetch_all(&state.db)
    .await?;
    let away: Vec<(String, i64)> = sqlx::query_as(
        "SELECT c.ai_category, COUNT(*)
         FROM classifications c JOIN email_messages m ON m.id = c.message_id
         WHERE m.workspace_id = ?1 AND m.direction = 'inbound' AND m.received_at >= ?2
           AND c.source = 'manual' AND c.ai_category IS NOT NULL AND c.ai_category != c.category
         GROUP BY c.ai_category",
    )
    .bind(&ws)
    .bind(since)
    .fetch_all(&state.db)
    .await?;
    let daily: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT c.category, m.received_at / ?3, COUNT(*)
         FROM classifications c JOIN email_messages m ON m.id = c.message_id
         WHERE m.workspace_id = ?1 AND m.direction = 'inbound' AND m.received_at >= ?2
         GROUP BY c.category, m.received_at / ?3",
    )
    .bind(&ws)
    .bind(since)
    .bind(DAY_MS)
    .fetch_all(&state.db)
    .await?;

    let total: i64 = stats.iter().map(|s| s.1).sum();
    let moved_in: i64 = stats.iter().map(|s| s.2).sum();
    let stats: HashMap<String, (i64, i64, Option<f64>)> = stats.into_iter().map(|(k, c, i, a)| (k, (c, i, a))).collect();
    let away: HashMap<String, i64> = away.into_iter().collect();
    let mut by_day: HashMap<String, HashMap<i64, i64>> = HashMap::new();
    for (key, day, n) in daily {
        by_day.entry(key).or_default().insert(day, n);
    }
    let trend_of = |key: &str| -> Vec<TrendPoint> {
        let counts = by_day.get(key);
        (first_day..=today)
            .map(|d| TrendPoint {
                date: iso(d * DAY_MS).chars().take(10).collect(),
                count: counts.and_then(|c| c.get(&d)).copied().unwrap_or(0),
            })
            .collect()
    };

    let mut out: Vec<LabelHealth> = Vec::new();
    let mut advice: Vec<Advice> = Vec::new();
    let mut other_count = 0;
    for (key, name, archived) in &labels {
        let (count, into, avg) = stats.get(key).copied().unwrap_or((0, 0, None));
        let corrected_away = away.get(key).copied().unwrap_or(0);
        if key == OTHER_LABEL {
            other_count = count;
            continue;
        }
        if *archived && count == 0 {
            continue;
        }
        let auto = count - into;
        let ai_picks = auto + corrected_away;
        if !*archived {
            if ai_picks >= UNRELIABLE_MIN_EMAILS && corrected_away as f64 / ai_picks as f64 >= UNRELIABLE_RATE {
                advice.push(Advice {
                    kind: "label_unreliable",
                    label_key: Some(key.clone()),
                    text: format!("People correct {name} often — sharpen its description."),
                });
            } else if count == 0 && corrected_away == 0 && total > 0 {
                advice.push(Advice {
                    kind: "label_unused",
                    label_key: Some(key.clone()),
                    text: format!("No email was labelled {name} in the last {days} days — is it still needed?"),
                });
            }
        }
        out.push(LabelHealth {
            key: key.clone(),
            name: name.clone(),
            count,
            share: ratio(count, total),
            auto,
            corrected_away,
            corrected_into: into,
            correction_rate: ratio(corrected_away, ai_picks),
            avg_confidence: avg.map(round2),
            trend: trend_of(key),
        });
    }

    let other_share = ratio(other_count, total);
    if total > 0 && other_count as f64 / total as f64 >= ADD_LABEL_SHARE {
        advice.insert(
            0,
            Advice {
                kind: "add_label",
                label_key: None,
                text: format!("{}% of mail lands in Other — try Suggest labels.", (other_share * 100.0).round() as i64),
            },
        );
    }

    Ok(Json(json!({
        "days": days,
        "total": total,
        "other_share": other_share,
        "auto_share": ratio(total - moved_in, total),
        "labels": out,
        "other_trend": trend_of(OTHER_LABEL),
        "advice": advice,
    })))
}
