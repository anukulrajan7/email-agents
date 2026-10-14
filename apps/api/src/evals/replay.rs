//! Replay: the agent's last approved or sent drafts are drafted again by the candidate version
//! (nothing is written or sent) and compared with what the person finally sent. Code graders check
//! that no new address appears and no ask is left unanswered; the judge scores closeness.

use serde_json::{Value, json};

use super::judge::closeness;
use crate::{
    agent_registry::spec::{AgentSpec, QualityTier},
    ai::{DraftInput, DraftQuality},
    ai_ledger,
    http::AppState,
    replies,
};

pub const MAX_CASES: i64 = 50;
/// A new version's replay may fall this far below the previous version's score.
pub const TOLERANCE: f64 = 0.02;

#[derive(Debug, Clone)]
pub struct CaseScore {
    pub key: String,
    pub passed: bool,
    pub score: Option<f64>,
    pub detail: Value,
}

#[derive(Debug, Clone, Default)]
pub struct ReplayResult {
    pub cases: Vec<CaseScore>,
    pub budget_hit: bool,
    pub judged: usize,
}

impl ReplayResult {
    /// Mean closeness over the cases that were scored.
    pub fn score(&self) -> Option<f64> {
        let scores: Vec<f64> = self.cases.iter().filter_map(|c| c.score).collect();
        (!scores.is_empty()).then(|| scores.iter().sum::<f64>() / scores.len() as f64)
    }
}

#[derive(sqlx::FromRow)]
struct Sample {
    id: String,
    final_text: String,
    subject: String,
    sender: String,
    incoming: String,
}

fn addresses(text: &str) -> Vec<String> {
    let mut out: Vec<String> = text
        .split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | ',' | ';' | '(' | ')' | '"'))
        .filter(|w| w.contains('@') && w.contains('.'))
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .collect();
    out.sort();
    out.dedup();
    out
}

pub async fn run(state: &AppState, ws: &str, agent_id: &str, spec: &AgentSpec) -> sqlx::Result<ReplayResult> {
    let samples: Vec<Sample> = sqlx::query_as(
        "SELECT d.id, d.body_text AS final_text, m.subject, m.sender_address AS sender, m.body_text AS incoming
         FROM drafts d JOIN email_messages m ON m.id = d.message_id
         WHERE d.workspace_id = ?1 AND d.agent_id = ?2 AND d.status IN ('approved','sent')
         ORDER BY d.updated_at DESC LIMIT ?3",
    )
    .bind(ws)
    .bind(agent_id)
    .bind(MAX_CASES)
    .fetch_all(&state.db)
    .await?;
    let mut rules = spec.drafting.rules.clone();
    rules.extend(spec.drafting.never_do.iter().map(|r| format!("Never: {r}")));
    let quality = if spec.drafting.quality_tier == QualityTier::Best { DraftQuality::Premium } else { DraftQuality::Standard };
    let mut result = ReplayResult::default();
    for s in samples {
        if !ai_ledger::within_budget(&state.db, state.config.ai_daily_budget_usd, ws).await {
            result.budget_hit = true;
            break;
        }
        let input = DraftInput {
            category: spec.category.clone(),
            category_name: spec.name.clone(),
            category_description: spec.description.clone(),
            subject: s.subject.clone(),
            sender: s.sender.clone(),
            body_text: s.incoming.clone(),
            memory_notes: vec![],
            sender_history: None,
            insight: None,
            examples: vec![],
            quality,
            reply_instructions: rules.join("\n"),
            signature: None,
            default_sign_off: String::new(),
        };
        let key = format!("draft:{}", s.id);
        let out = match state.drafter.draft(&input).await {
            Ok(out) => out,
            Err(e) => {
                result.cases.push(CaseScore { key, passed: false, score: None, detail: json!({"error": e.to_string()}) });
                continue;
            }
        };
        let known: Vec<String> = [&s.final_text, &s.incoming, &s.sender].iter().flat_map(|t| addresses(t)).collect();
        let new_recipients: Vec<String> = addresses(&out.body_text).into_iter().filter(|a| !known.contains(a)).collect();
        let unanswered = !out.unanswered.is_empty() && !replies::has_open_gaps(&s.final_text);
        let close = closeness(state.judge.as_ref(), &s.final_text, &out.body_text).await;
        if close.judged {
            result.judged += 1;
        }
        result.cases.push(CaseScore {
            key,
            passed: new_recipients.is_empty() && !unanswered,
            score: Some(close.score),
            detail: json!({"new_recipients": new_recipients.len(), "unanswered_asks": unanswered, "judged": close.judged}),
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_found_in_prose() {
        assert_eq!(addresses("Mail Ravi <Ravi@Client.com>, cc ops@x.io."), ["ops@x.io", "ravi@client.com"]);
        assert!(addresses("no address here, just @mention").is_empty());
    }
}
