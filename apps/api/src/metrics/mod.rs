//! Per-agent metrics: a rollup of cases, approvals, drafts, routes, audit events and the AI ledger
//! into `agent_metrics_daily`, and the queries that turn it into a scorecard.

pub mod queries;
pub mod rollup;

use serde::Serialize;

pub const DAY_MS: i64 = 86_400_000;

/// Days since the Unix epoch (UTC).
pub fn day_of(ms: i64) -> i64 {
    ms.div_euclid(DAY_MS)
}

/// `YYYY-MM-DD` for a day number.
pub fn day_label(day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{y:04}-{m:02}-{d:02}")
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Word-level Levenshtein distance divided by the longer text: 0 = unchanged, 1 = rewritten.
pub fn edit_distance(before: &str, after: &str) -> f64 {
    let a: Vec<&str> = before.split_whitespace().collect();
    let b: Vec<&str> = after.split_whitespace().collect();
    if a == b {
        return 0.0;
    }
    let longest = a.len().max(b.len());
    if a.is_empty() || b.is_empty() {
        return 1.0;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, wa) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, wb) in b.iter().enumerate() {
            let cost = usize::from(wa != wb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()] as f64 / longest as f64
}

/// Nearest-rank percentile of unsorted values; `None` when empty.
pub fn percentile(values: &[f64], p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * v.len() as f64).ceil().max(1.0) as usize;
    Some(v[rank.min(v.len()) - 1])
}

pub fn ratio(n: i64, d: i64) -> Option<f64> {
    (d > 0).then(|| n as f64 / d as f64)
}

pub fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

/// Summed counters over a window of days.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Counts {
    pub routed: i64,
    pub cases: i64,
    pub done: i64,
    pub failed: i64,
    pub cancelled: i64,
    pub approvals_requested: i64,
    pub approved: i64,
    pub edited: i64,
    pub declined: i64,
    pub drafts_created: i64,
    pub drafts_sent: i64,
    pub edit_sum: f64,
    pub edit_n: i64,
    pub auto_sends: i64,
    pub stopped: i64,
    pub incidents: i64,
    pub label_corrections: i64,
    pub labels_total: i64,
    pub shadow_sum: f64,
    pub shadow_n: i64,
    pub cost_usd: f64,
}

impl Counts {
    pub fn decided(&self) -> i64 {
        self.approved + self.declined
    }
    pub fn approval_rate(&self) -> Option<f64> {
        ratio(self.approved, self.decided())
    }
}

/// What the scorecard shows for one agent over a window.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Totals {
    pub routed: i64,
    pub cases: i64,
    pub success_rate: Option<f64>,
    pub approval_rate: Option<f64>,
    pub decline_rate: Option<f64>,
    pub approvals_decided: i64,
    pub edit_distance_median: Option<f64>,
    pub auto_sends: i64,
    pub stop_rate: Option<f64>,
    pub label_accuracy: Option<f64>,
    pub shadow_agreement: Option<f64>,
    pub cost_usd: f64,
    pub cost_per_email_usd: Option<f64>,
    pub cost_per_case_usd: Option<f64>,
    pub latency_p50_ms: Option<i64>,
    pub latency_p95_ms: Option<i64>,
    pub incidents: i64,
}

impl Totals {
    pub fn from_counts(c: &Counts, edit_median: Option<f64>, latencies_ms: &[f64]) -> Self {
        let decided = c.decided();
        Totals {
            routed: c.routed,
            cases: c.cases,
            success_rate: ratio(c.done, c.done + c.failed),
            approval_rate: c.approval_rate(),
            decline_rate: ratio(c.declined, decided),
            approvals_decided: decided,
            edit_distance_median: edit_median.map(round4),
            auto_sends: c.auto_sends,
            stop_rate: ratio(c.stopped, c.auto_sends),
            label_accuracy: (c.labels_total > 0).then(|| 1.0 - c.label_corrections as f64 / c.labels_total as f64),
            shadow_agreement: (c.shadow_n > 0).then(|| c.shadow_sum / c.shadow_n as f64),
            cost_usd: (c.cost_usd * 1e6).round() / 1e6,
            cost_per_email_usd: (c.routed > 0).then(|| c.cost_usd / c.routed as f64).map(|v| (v * 1e6).round() / 1e6),
            cost_per_case_usd: (c.cases > 0).then(|| c.cost_usd / c.cases as f64).map(|v| (v * 1e6).round() / 1e6),
            latency_p50_ms: percentile(latencies_ms, 50.0).map(|v| v as i64),
            latency_p95_ms: percentile(latencies_ms, 95.0).map(|v| v as i64),
            incidents: c.incidents,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_distance_is_word_level_and_normalised() {
        assert_eq!(edit_distance("a b c", "a b c"), 0.0);
        assert_eq!(edit_distance("a b c d", "a b x d"), 0.25);
        assert_eq!(edit_distance("a b", ""), 1.0);
        assert_eq!(edit_distance("a  b\nc", "a b c"), 0.0, "whitespace does not count");
        assert!((edit_distance("one two three four", "five six seven eight") - 1.0).abs() < 1e-9);
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let v: Vec<f64> = (1..=20).map(f64::from).collect();
        assert_eq!(percentile(&v, 50.0), Some(10.0));
        assert_eq!(percentile(&v, 95.0), Some(19.0));
        assert_eq!(percentile(&[], 50.0), None);
        assert_eq!(percentile(&[7.0], 95.0), Some(7.0));
    }

    #[test]
    fn day_labels_are_utc_dates() {
        assert_eq!(day_label(0), "1970-01-01");
        assert_eq!(day_label(day_of(1_776_124_800_000)), "2026-04-14");
        assert_eq!(day_label(20_000), "2024-10-04");
    }
}
