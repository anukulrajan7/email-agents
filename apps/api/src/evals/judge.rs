//! How close a candidate reply is to what a person finally sent. The judge model scores facts,
//! commitments and tone; when it is unavailable the score falls back to word overlap, so a run
//! never silently scores everything as perfect.

use crate::{
    ai::judge::{Judge, Question},
    metrics::edit_distance,
};

const QUESTION: &str = "How close is the CANDIDATE reply to the FINAL reply a person sent? Compare the facts it states, the \
commitments it makes and its tone. 0 = unrelated or contradicts it, 1 = says the same thing the same way.";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Closeness {
    pub score: f64,
    pub judged: bool,
}

pub async fn closeness(judge: &dyn Judge, final_text: &str, candidate: &str) -> Closeness {
    let context = format!("FINAL reply:\n{final_text}\n\nCANDIDATE reply:\n{candidate}");
    if let Ok(answers) = judge.judge(&context, &[Question::score("closeness", QUESTION)]).await
        && let Some(score) = answers.score("closeness")
    {
        return Closeness { score: score.clamp(0.0, 1.0), judged: true };
    }
    Closeness { score: (1.0 - edit_distance(final_text, candidate)).clamp(0.0, 1.0), judged: false }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::judge::{FixedJudge, Verdict};

    #[tokio::test]
    async fn judge_score_wins_and_overlap_is_the_fallback() {
        let judge = FixedJudge::new().with("closeness", Verdict::Score(0.8), "");
        assert_eq!(closeness(&judge, "a b c d", "x y z").await, Closeness { score: 0.8, judged: true });
        judge.fail(true);
        let c = closeness(&judge, "a b c d", "a b c x").await;
        assert!(!c.judged);
        assert!((c.score - 0.75).abs() < 1e-9);
    }
}
