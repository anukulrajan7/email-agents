//! How the safety check gets through a provider outage without ever stopping a send for good:
//! retry twice with a short pause, try a fallback model, then defer 5 / 10 / 20 minutes, then ask a
//! person. `next` is the pure ladder; `run` walks its in-call rungs; the callers own the deferrals
//! (the runner re-queues the same turn, the scheduled send re-queues itself).

use std::{
    future::Future,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

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
const DEFER_MINUTES: [i64; 3] = [5, 10, 20];
/// All in-call rungs together stop here; the job's lease heartbeat keeps a long turn from being claimed twice.
const DEADLINE: Duration = Duration::from_secs(90);

/// What to do after attempt number `attempt` (counted from 0 across the whole ladder) failed with a
/// retryable error. `base_ms` is the first pause (the second is 3x); 0 means no pauses.
pub fn next(attempt: u32, base_ms: u64) -> Step {
    match attempt {
        a if a < RETRIES => Step::Retry(base_ms * BACKOFF_STEPS[a as usize]),
        a if a == RETRIES => Step::Fallback,
        a => match DEFER_MINUTES.get((a - RETRIES - 1) as usize) {
            Some(minutes) => Step::Defer(*minutes),
            None => Step::Ask,
        },
    }
}

/// The rung after `deferrals` earlier deferrals of the same check: another wait, or a person.
pub fn after_deferrals(deferrals: u32) -> Step {
    next(RETRIES + 1 + deferrals, 0)
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
    /// The whole in-call ladder gives up (as a retryable error) after this long.
    pub deadline: Duration,
    /// One call is cut off (as a retryable error) after this long, whatever the HTTP client allows.
    pub attempt_timeout: Duration,
}

impl Ladder {
    pub fn from_config(config: &Config) -> Self {
        Self {
            fallback_model: config.openrouter_safety_fallback_model_id.clone(),
            backoff_ms: config.ai_retry_backoff_ms,
            deadline: DEADLINE,
            attempt_timeout: Duration::from_secs(config.ai_attempt_timeout_secs),
        }
    }

    /// For a check a person is waiting on in the editor: it gives up after `AI_PERSON_CHECK_SECS` (15) and the UI shows "AI review unavailable".
    pub fn for_person(config: &Config) -> Self {
        let wait = Duration::from_secs(config.ai_person_check_secs);
        Self { deadline: wait, attempt_timeout: wait, ..Self::from_config(config) }
    }
}

/// Calls `call` (with `None` for the usual model, `Some(model)` for the fallback) and walks the in-call
/// rungs while it fails with a retryable error. A permanent error from the usual model returns at once.
/// The fallback rung is best effort: whatever goes wrong there (no provider, bad key, bad answer) only
/// means the rung was unusable, and the usual model's retryable error comes back so the caller defers.
/// The same happens when the whole ladder outlasts its deadline.
pub async fn run<T, F, Fut>(ladder: &Ladder, mut call: F) -> Result<T, AiError>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = Result<T, AiError>>,
{
    let walk = async {
        let mut attempt = 0;
        let mut model: Option<String> = None;
        let mut primary_why = String::new();
        loop {
            let result = tokio::time::timeout(ladder.attempt_timeout, call(model.clone()))
                .await
                .unwrap_or_else(|_| Err(AiError::Retryable("the safety check call timed out".into())));
            if model.is_some() {
                return match result {
                    Ok(v) => Ok(v),
                    Err(e) => {
                        tracing::warn!(error = %e, model = ?model, "the fallback model could not answer the safety check");
                        Err(AiError::Retryable(primary_why))
                    }
                };
            }
            let why = match result {
                Err(AiError::Retryable(why)) => why,
                done => return done,
            };
            match next(attempt, ladder.backoff_ms) {
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
            primary_why = why;
            attempt += 1;
        }
    };
    tokio::time::timeout(ladder.deadline, walk).await.unwrap_or_else(|_| Err(AiError::Retryable("the safety check ran out of time".into())))
}

/// A judge that rides the ladder, so one cached `ask` retries and falls back like the critic does.
/// An answer from the fallback model is a stand-in for one call only: it is never cached.
pub struct LadderJudge<'a> {
    inner: &'a dyn Judge,
    ladder: &'a Ladder,
    fell_back: AtomicBool,
}

impl<'a> LadderJudge<'a> {
    pub fn new(inner: &'a dyn Judge, ladder: &'a Ladder) -> Self {
        Self { inner, ladder, fell_back: AtomicBool::new(false) }
    }
}

#[async_trait]
impl Judge for LadderJudge<'_> {
    async fn judge(&self, context: &str, questions: &[Question]) -> Result<Answers, AiError> {
        run(self.ladder, |model| async move {
            let answers = self.inner.judge_with(context, questions, model.as_deref()).await;
            if model.is_some() && answers.is_ok() {
                self.fell_back.store(true, Ordering::SeqCst);
            }
            answers
        })
        .await
    }

    fn cacheable(&self) -> bool {
        !self.fell_back.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn ladder_is_retry_retry_fallback_then_defer_5_10_20_then_ask() {
        let steps: Vec<Step> = (0..7).map(|a| next(a, 1_000)).collect();
        assert_eq!(
            steps,
            [Step::Retry(1_000), Step::Retry(3_000), Step::Fallback, Step::Defer(5), Step::Defer(10), Step::Defer(20), Step::Ask]
        );
        assert_eq!(next(40, 1_000), Step::Ask);
        assert_eq!((0..4).map(after_deferrals).collect::<Vec<_>>(), [Step::Defer(5), Step::Defer(10), Step::Defer(20), Step::Ask]);
        assert_eq!(outage_minutes(), 35);
    }

    fn ladder(fallback: &str) -> Ladder {
        Ladder { fallback_model: fallback.into(), backoff_ms: 0, deadline: Duration::from_secs(5), attempt_timeout: Duration::from_secs(5) }
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
        assert!(matches!(out, Err(AiError::Retryable(m)) if m == "c"), "the caller defers on the usual model's error");
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

    #[tokio::test]
    async fn a_failing_fallback_of_any_kind_returns_the_primary_retryable_so_the_caller_defers() {
        for fallback_error in [AiError::Permanent("404 no provider".into()), AiError::Retryable("429".into())] {
            let (out, seen) = scripted(vec![down("a"), down("b"), down("primary 429"), Err(fallback_error)], &ladder("fb")).await;
            assert!(matches!(&out, Err(AiError::Retryable(m)) if m == "primary 429"), "{out:?}");
            assert_eq!(seen.len(), 4);
        }
    }

    #[tokio::test]
    async fn the_whole_ladder_gives_up_at_its_deadline_as_retryable() {
        let ladder = Ladder { fallback_model: "fb".into(), backoff_ms: 0, deadline: Duration::from_millis(50), attempt_timeout: Duration::from_secs(5) };
        let started = std::time::Instant::now();
        let out: Result<u32, AiError> = run(&ladder, |_| async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(1)
        })
        .await;
        assert!(matches!(out, Err(AiError::Retryable(_))), "{out:?}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn a_hanging_call_times_out_per_attempt_so_the_fallback_is_reached_before_the_deadline() {
        let ladder = Ladder { fallback_model: "fb".into(), backoff_ms: 0, deadline: Duration::from_secs(5), attempt_timeout: Duration::from_millis(30) };
        let seen = Mutex::new(vec![]);
        let out: Result<u32, AiError> = run(&ladder, |model| {
            seen.lock().unwrap().push(model.clone());
            async move {
                if model.is_none() {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
                Ok(5)
            }
        })
        .await;
        assert_eq!(out.unwrap(), 5);
        assert_eq!(seen.into_inner().unwrap(), [None, None, None, Some("fb".to_string())]);
    }

    #[test]
    fn the_ladder_fits_its_deadline_and_a_person_waits_less() {
        let config = Config::for_tests("sqlite://unused");
        let automated = Ladder::from_config(&config);
        assert_eq!(automated.attempt_timeout, Duration::from_secs(20), "the default");
        let tuned = Config { ai_attempt_timeout_secs: 7, ..config.clone() };
        assert_eq!(Ladder::from_config(&tuned).attempt_timeout, Duration::from_secs(7));
        let worst_case = automated.attempt_timeout * (RETRIES + 2) + Duration::from_secs(4);
        assert!(worst_case <= automated.deadline, "{worst_case:?} must fit {:?}", automated.deadline);
        let person = Ladder::for_person(&config);
        assert_eq!(person.deadline, Duration::from_secs(15));
        assert!(person.deadline < automated.deadline);
    }
}
