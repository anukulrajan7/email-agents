//! How the safety check gets through a provider outage without ever stopping a send for good:
//! retry twice with a short pause, try a fallback model, then defer 5 / 10 / 20 minutes, then ask a
//! person. `next` is the pure ladder; `run` walks its in-call rungs; the callers own the deferrals
//! (the runner re-queues the same turn, the scheduled send re-queues itself).

use std::future::Future;
use std::time::Duration;

use async_trait::async_trait;

use super::{
    AiError,
    judge::{Answers, Judge, Question},
};
use crate::platform::config::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Call the same model again after this many milliseconds.
    Retry(u64),
    /// Call the fallback model once.
    Fallback,
    /// Give up for now; look again after this many minutes.
    Defer(i64),
    /// Out of rungs: a person reads it.
    Ask,
}

const RETRIES: u32 = 2;
const BACKOFF_STEPS: [u64; RETRIES as usize] = [1, 3];
const MAX_RETRY_WAIT_MS: u64 = 10_000;
const DEFER_MINUTES: [i64; 3] = [5, 10, 20];

/// What to do after attempt number `attempt` (counted from 0 across the whole ladder) failed with a
/// retryable error. `base_ms` is the first pause (the second is 3x); 0 means no pauses. A provider's
/// `Retry-After` is honoured when it asks for longer, up to ten seconds.
pub fn next(attempt: u32, base_ms: u64, retry_after_ms: Option<u64>) -> Step {
    match attempt {
        a if a < RETRIES => {
            let wait = base_ms * BACKOFF_STEPS[a as usize];
            let asked = if base_ms == 0 { 0 } else { retry_after_ms.unwrap_or(0) };
            Step::Retry(wait.max(asked).min(MAX_RETRY_WAIT_MS.max(wait)))
        }
        a if a == RETRIES => Step::Fallback,
        a => match DEFER_MINUTES.get((a - RETRIES - 1) as usize) {
            Some(minutes) => Step::Defer(*minutes),
            None => Step::Ask,
        },
    }
}

/// The rung after `deferrals` earlier deferrals of the same check: another wait, or a person.
pub fn after_deferrals(deferrals: u32) -> Step {
    next(RETRIES + 1 + deferrals, 0, None)
}

/// Minutes a check has been out when every deferral has been used.
pub fn outage_minutes() -> i64 {
    DEFER_MINUTES.iter().sum()
}

#[derive(Debug, Clone)]
pub struct Ladder {
    /// Empty = no fallback rung.
    pub fallback_model: String,
    pub backoff_ms: u64,
}

impl Ladder {
    pub fn from_config(config: &Config) -> Self {
        Self { fallback_model: config.openrouter_safety_fallback_model_id.clone(), backoff_ms: config.ai_retry_backoff_ms }
    }
}

/// Calls `call` (with `None` for the usual model, `Some(model)` for the fallback) and walks the in-call
/// rungs while it fails with a retryable error. Permanent errors return at once. The last retryable
/// error comes back when the rungs are used up, and the caller defers.
pub async fn run<T, F, Fut>(ladder: &Ladder, mut call: F) -> Result<T, AiError>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = Result<T, AiError>>,
{
    let mut attempt = 0;
    let mut model: Option<String> = None;
    loop {
        let why = match call(model.clone()).await {
            Err(AiError::Retryable(why)) => why,
            done => return done,
        };
        match next(attempt, ladder.backoff_ms, None) {
            Step::Retry(ms) => {
                tracing::warn!(attempt, error = %why, "safety check failed; retrying");
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
            Step::Fallback if !ladder.fallback_model.is_empty() => {
                tracing::warn!(error = %why, model = %ladder.fallback_model, "safety check failed; trying the fallback model");
                model = Some(ladder.fallback_model.clone());
            }
            _ => return Err(AiError::Retryable(why)),
        }
        attempt += 1;
    }
}

/// A judge that rides the ladder, so one cached `ask` retries and falls back like the critic does.
pub struct LadderJudge<'a> {
    pub inner: &'a dyn Judge,
    pub ladder: &'a Ladder,
}

#[async_trait]
impl Judge for LadderJudge<'_> {
    async fn judge(&self, context: &str, questions: &[Question]) -> Result<Answers, AiError> {
        run(self.ladder, |model| async move { self.inner.judge_with(context, questions, model.as_deref()).await }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn ladder_is_retry_retry_fallback_then_defer_5_10_20_then_ask() {
        let steps: Vec<Step> = (0..7).map(|a| next(a, 1_000, None)).collect();
        assert_eq!(
            steps,
            [Step::Retry(1_000), Step::Retry(3_000), Step::Fallback, Step::Defer(5), Step::Defer(10), Step::Defer(20), Step::Ask]
        );
        assert_eq!(next(40, 1_000, None), Step::Ask);
        assert_eq!((0..4).map(after_deferrals).collect::<Vec<_>>(), [Step::Defer(5), Step::Defer(10), Step::Defer(20), Step::Ask]);
        assert_eq!(outage_minutes(), 35);
    }

    #[test]
    fn retry_after_is_honoured() {
        assert_eq!(next(0, 1_000, Some(2_500)), Step::Retry(2_500));
        assert_eq!(next(1, 1_000, Some(500)), Step::Retry(3_000), "the longer of the two");
        assert_eq!(next(0, 1_000, Some(60_000)), Step::Retry(10_000), "bounded");
        assert_eq!(next(0, 0, Some(2_500)), Step::Retry(0), "no pauses means none, tests included");
    }

    fn ladder(fallback: &str) -> Ladder {
        Ladder { fallback_model: fallback.into(), backoff_ms: 0 }
    }

    async fn scripted(script: Vec<Result<u32, AiError>>, ladder: &Ladder) -> (Result<u32, AiError>, Vec<Option<String>>) {
        let script = Mutex::new(script.into_iter());
        let seen = Mutex::new(vec![]);
        let out = run(ladder, |model| {
            seen.lock().unwrap().push(model);
            let next = script.lock().unwrap().next().expect("no more calls than scripted");
            async move { next }
        })
        .await;
        (out, seen.into_inner().unwrap())
    }

    fn down(why: &str) -> Result<u32, AiError> {
        Err(AiError::Retryable(why.into()))
    }

    #[tokio::test]
    async fn two_retries_then_ok_never_reaches_the_fallback() {
        let (out, seen) = scripted(vec![down("429"), down("429"), Ok(7)], &ladder("fb")).await;
        assert_eq!(out.unwrap(), 7);
        assert_eq!(seen, [None, None, None]);
    }

    #[tokio::test]
    async fn the_fallback_model_is_called_once_after_the_retries() {
        let (out, seen) = scripted(vec![down("429"), down("503"), down("timeout"), Ok(9)], &ladder("fb")).await;
        assert_eq!(out.unwrap(), 9);
        assert_eq!(seen, [None, None, None, Some("fb".to_string())]);
        let (out, seen) = scripted(vec![down("a"), down("b"), down("c"), down("d")], &ladder("fb")).await;
        assert!(matches!(out, Err(AiError::Retryable(m)) if m == "d"), "the caller defers");
        assert_eq!(seen.len(), 4);
    }

    #[tokio::test]
    async fn without_a_fallback_model_the_ladder_ends_after_the_retries() {
        let (out, seen) = scripted(vec![down("a"), down("b"), down("c")], &ladder("")).await;
        assert!(matches!(out, Err(AiError::Retryable(_))));
        assert_eq!(seen.len(), 3);
    }

    #[tokio::test]
    async fn a_permanent_error_returns_at_once() {
        let (out, seen) = scripted(vec![Err(AiError::Permanent("404".into()))], &ladder("fb")).await;
        assert!(matches!(out, Err(AiError::Permanent(_))));
        assert_eq!(seen.len(), 1);
    }
}
