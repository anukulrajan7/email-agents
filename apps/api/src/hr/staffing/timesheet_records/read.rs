use super::*;

fn mismatch(sheet: &Sheet, tolerance: f64) -> bool {
    match (sheet.total, sheet.entries.is_empty()) {
        (Some(total), false) => (sheet.entries.iter().map(|e| e.hours).sum::<f64>() - total).abs() > tolerance,
        _ => false,
    }
}

/// A month or period the text really names: both end dates are written, or it is the month named in the text or subject.
fn period_supported(period: &(String, String), text: &str, subject: &str, hint_year: i64) -> bool {
    let written: HashSet<String> = text.lines().flat_map(|l| parse::dates_in(l, hint_year)).collect();
    (written.contains(&period.0) && written.contains(&period.1)) || parse::month_named(text, subject, hint_year).as_ref() == Some(period)
}

/// Keeps only what the text itself states: a model may copy but not invent. A day stays only when its date is written
/// in the text and its hours stand on the same line; the period must be written too; a total must be a stated number.
fn sanitize_model(mut s: Sheet, text: &str, subject: &str, hint_year: i64) -> (Sheet, Vec<Issue>) {
    let mut issues = vec![];
    let numbers = parse::numbers_in(text);
    let stated = |n: f64| numbers.iter().any(|x| parse::same_hours(*x, n));
    let lines: Vec<(Vec<String>, Vec<f64>)> =
        text.lines().filter(|l| l.chars().any(|c| c.is_ascii_digit())).map(|l| (parse::dates_in(l, hint_year), parse::numbers_in(l))).collect();
    let before = s.entries.len();
    s.entries.retain(|e| match &e.date {
        Some(d) => lines.iter().any(|(dates, nums)| dates.contains(d) && nums.iter().any(|n| parse::same_hours(*n, e.hours))),
        None => stated(e.hours),
    });
    let dropped = before - s.entries.len();
    if dropped > 0 {
        issues.push(check("model_days_dropped", format!("{dropped} day(s) the model returned are not in the sheet's text and were left out.")));
    }
    if s.period.as_ref().is_some_and(|p| !period_supported(p, text, subject, hint_year)) {
        s.period = None;
    }
    if s.total.is_some_and(|t| !stated(t)) {
        s.total = None;
    }
    let lower = text.to_lowercase();
    let written = |v: &Option<String>| v.as_ref().is_some_and(|v| lower.contains(&v.to_lowercase()));
    for field in [&mut s.identity.code, &mut s.identity.name, &mut s.identity.email, &mut s.identity.manager, &mut s.identity.client, &mut s.identity.location] {
        if !written(field) {
            *field = None;
        }
    }
    (s, issues)
}

/// The rules' reading wins wherever it found something: its total is never replaced by the model's.
fn merge(mut base: Sheet, model: Sheet) -> Sheet {
    let id = &mut base.identity;
    id.code = id.code.take().or(model.identity.code);
    id.name = id.name.take().or(model.identity.name);
    id.email = id.email.take().or(model.identity.email);
    id.client = id.client.take().or(model.identity.client);
    id.manager = id.manager.take().or(model.identity.manager);
    id.location = id.location.take().or(model.identity.location);
    base.period = model.period.or(base.period);
    base.total = base.total.or(model.total);
    if !model.entries.is_empty() {
        base.entries = model.entries;
    }
    base
}


/// Plain rules first. The model reads only what they could not: nothing found, or entries that do not add up to
/// the stated total. Cheap model, then the strong one when the cheap answer still fails or the sheet is large.
pub(super) async fn read_text(state: &AppState, ws: &str, subject: &str, text: &str, hint_year: i64, tolerance: f64, allow_model: bool) -> Read {
    let mut out = Read { sheet: parse::parse_sheet(text, subject, hint_year), cost: 0.0, issues: vec![], deferred: false };
    let bad = |s: &Sheet| !s.has_data() || mismatch(s, tolerance);
    if !allow_model || !bad(&out.sheet) || !text.chars().any(|c| c.is_ascii_digit()) {
        return out;
    }
    let (chars, rows) = (text.chars().count(), text.lines().count());
    let wanted: &[SheetTier] = if sheet_tier(chars, rows, false) == SheetTier::Strong { &[SheetTier::Strong] } else { &[SheetTier::Cheap, SheetTier::Strong] };
    let tiers: Vec<SheetTier> = wanted.iter().copied().filter(|t| state.timesheet_reader.has(*t)).collect();
    if tiers.is_empty() {
        return out;
    }
    if !crate::ai::ledger::within_budget(&state.db, state.config.ai_daily_budget_usd, ws).await {
        out.deferred = !out.sheet.has_data();
        return out;
    }
    let rules = out.sheet.clone();
    let mut model_issues: Vec<Issue> = vec![];
    let mut from_model = false;
    for tier in tiers {
        match state.timesheet_reader.read(tier, subject, text, hint_year).await {
            Ok(m) => {
                out.cost += m.cost_usd;
                let (clean, dropped) = sanitize_model(m.sheet, text, subject, hint_year);
                let uses_model_days = !clean.entries.is_empty();
                let candidate = merge(rules.clone(), clean);
                let better = !bad(&candidate) || (!out.sheet.has_data() && candidate.has_data());
                if better {
                    out.sheet = candidate;
                    model_issues = dropped;
                    from_model = uses_model_days;
                }
                if !bad(&out.sheet) {
                    break;
                }
            }
            Err(AiError::Retryable(e)) => {
                tracing::warn!(error = %e, %ws, "timesheet model is not available just now; the sheet is read again later");
                out.deferred = true;
                return out;
            }
            Err(e) => tracing::warn!(error = %e, %ws, "timesheet model could not read the sheet"),
        }
    }
    out.issues = model_issues;
    let sum: f64 = out.sheet.entries.iter().map(|e| e.hours).sum();
    if from_model && !out.sheet.total.is_some_and(|t| parse::same_hours(sum, t)) {
        out.issues.push(check("model_read", "The days were read by an AI model and do not add up to a total the sheet states; check them against the sheet."));
    }
    out
}

pub(super) fn weekday_name(d: i64) -> &'static str {
    ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][d.rem_euclid(7) as usize]
}

fn nice_date(d: &str) -> String {
    let (m, day) = (&d[5..7], &d[8..10]);
    let month = parse::MONTH_NAMES.get(m.parse::<usize>().unwrap_or(1).saturating_sub(1)).copied().unwrap_or("");
    format!("{} {month}", day.trim_start_matches('0'))
}

/// Total to keep and the issues found. Missing things stay missing; nothing is filled in.
pub(super) fn assess(sheet: &Sheet, period: &(String, String), period_assumed: bool, tolerance: f64, matched: bool, who_text: &str) -> (Option<f64>, Vec<Issue>) {
    let mut issues = vec![];
    let sum: f64 = sheet.entries.iter().map(|e| e.hours).sum();
    let total = match (sheet.total, sheet.entries.is_empty()) {
        (Some(t), _) => Some(t),
        (None, false) => {
            issues.push(note("total_added_up", "The sheet states no total; this is the sum of its days."));
            Some((sum * 100.0).round() / 100.0)
        }
        (None, true) => None,
    };
    if !sheet.has_data() {
        issues.push(check("no_hours", "No hours could be read from this timesheet."));
    }
    if mismatch(sheet, tolerance) {
        issues.push(check("total_mismatch", format!("The days add up to {sum} hours but the sheet says {}.", total.unwrap_or_default())));
    }
    if period_assumed && sheet.has_data() {
        issues.push(note("period_assumed", "The sheet names no period; the month of the email was used."));
    }
    if !matched {
        issues.push(check("person_unmatched", format!("Who is this? ({who_text})")));
    }
    let dated: Vec<(&Entry, &str)> = sheet.entries.iter().filter_map(|e| e.date.as_deref().map(|d| (e, d))).collect();
    let outside: Vec<&str> = dated.iter().filter(|(_, d)| *d < period.0.as_str() || *d > period.1.as_str()).map(|(_, d)| *d).collect();
    if !outside.is_empty() {
        issues.push(note("outside_period", format!("{} day(s) fall outside the period, e.g. {}.", outside.len(), nice_date(outside[0]))));
    }
    let impossible: Vec<String> = sheet.entries.iter().filter(|e| e.hours > 24.0).map(|e| e.date.as_deref().map(nice_date).unwrap_or_else(|| "a row".into())).collect();
    if !impossible.is_empty() {
        issues.push(check("impossible_hours", format!("More than 24 hours on {}.", impossible.join(", "))));
    }
    let mut per_day: HashMap<&str, f64> = HashMap::new();
    for (e, d) in &dated {
        *per_day.entry(d).or_default() += e.hours;
    }
    let mut long: Vec<&str> = per_day.iter().filter(|(_, h)| **h > 12.0 && **h <= 24.0).map(|(d, _)| *d).collect();
    long.sort();
    if !long.is_empty() {
        issues.push(note("long_days", format!("More than 12 hours on {}.", long.iter().take(4).map(|d| nice_date(d)).collect::<Vec<_>>().join(", "))));
    }
    let mut weekend: Vec<String> = per_day
        .iter()
        .filter(|(d, h)| **h > 0.0 && parse::weekday(d).is_some_and(|w| w >= 5))
        .map(|(d, _)| format!("{} ({})", nice_date(d), weekday_name(parse::weekday(d).unwrap_or(0))))
        .collect();
    weekend.sort();
    if !weekend.is_empty() {
        issues.push(note("weekend_hours", format!("Hours on a weekend: {}.", weekend.iter().take(4).cloned().collect::<Vec<_>>().join(", "))));
    }
    if dated.len() >= 5
        && let (Some(a), Some(b)) = (clock::parse_date(&period.0), clock::parse_date(&period.1))
        && b - a <= 31
    {
        let worked: HashSet<&str> = per_day.keys().copied().collect();
        let missing: Vec<String> = (a..=b)
            .map(clock::date_string)
            .filter(|d| parse::weekday(d).is_some_and(|w| w < 5) && !worked.contains(d.as_str()))
            .collect();
        if !missing.is_empty() {
            let shown: Vec<String> = missing.iter().take(5).map(|d| nice_date(d)).collect();
            issues.push(note("missing_days", format!("No hours for {} weekday(s): {}{}.", missing.len(), shown.join(", "), if missing.len() > 5 { ", ..." } else { "" })));
        }
    }
    (total, issues)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hr::staffing::timesheet_parse::Identity;

    fn sheet(entries: &[(&str, f64)], total: Option<f64>) -> Sheet {
        Sheet {
            identity: Identity::default(),
            period: None,
            total,
            entries: entries.iter().map(|(d, h)| Entry { date: Some((*d).into()), hours: *h, project: None, note: None }).collect(),
        }
    }

    fn period() -> (String, String) {
        ("2026-09-01".into(), "2026-09-30".into())
    }

    #[test]
    fn days_that_do_not_add_up_need_a_check() {
        let s = sheet(&[("2026-09-01", 8.0), ("2026-09-02", 8.0)], Some(20.0));
        let (total, issues) = assess(&s, &period(), false, 0.5, true, "x");
        assert_eq!(total, Some(20.0));
        assert!(issues.iter().any(|i| i.code == "total_mismatch" && i.severity == CHECK));
        let ok = sheet(&[("2026-09-01", 8.0), ("2026-09-02", 8.0)], Some(16.0));
        assert!(!assess(&ok, &period(), false, 0.5, true, "x").1.iter().any(|i| i.severity == CHECK));
    }

    #[test]
    fn a_missing_total_is_added_up_with_a_note_and_nothing_is_invented() {
        let s = sheet(&[("2026-09-01", 8.0), ("2026-09-02", 7.5)], None);
        let (total, issues) = assess(&s, &period(), false, 0.5, true, "x");
        assert_eq!(total, Some(15.5));
        assert!(issues.iter().any(|i| i.code == "total_added_up" && i.severity == NOTE));
        let (total, issues) = assess(&Sheet::default(), &period(), true, 0.5, true, "x");
        assert_eq!(total, None);
        assert!(issues.iter().any(|i| i.code == "no_hours" && i.severity == CHECK));
    }

    #[test]
    fn weekends_long_days_and_missing_weekdays_are_noted_not_blocking() {
        let s = sheet(&[("2026-09-01", 8.0), ("2026-09-02", 13.0), ("2026-09-03", 8.0), ("2026-09-05", 4.0), ("2026-09-04", 8.0)], Some(41.0));
        let (_, issues) = assess(&s, &period(), false, 0.5, true, "x");
        let codes: Vec<&str> = issues.iter().map(|i| i.code.as_str()).collect();
        assert!(codes.contains(&"weekend_hours") && codes.contains(&"long_days") && codes.contains(&"missing_days"), "{codes:?}");
        assert!(issues.iter().all(|i| i.severity == NOTE));
    }

    #[test]
    fn an_unmatched_person_is_a_question_for_a_person() {
        let s = sheet(&[("2026-09-01", 8.0)], Some(8.0));
        let (_, issues) = assess(&s, &period(), false, 0.5, false, "Ravi Kumar, ID E1");
        let q = issues.iter().find(|i| i.code == "person_unmatched").unwrap();
        assert_eq!((q.severity, q.message.as_str()), (CHECK, "Who is this? (Ravi Kumar, ID E1)"));
    }

    fn entry(date: &str, hours: f64) -> Entry {
        Entry { date: Some(date.into()), hours, project: None, note: None }
    }

    #[test]
    fn a_model_cannot_add_numbers_the_sheet_does_not_state() {
        let text = "Ravi Kumar\n01 Sep 8\n02 Sep 8\nTotal 16";
        let model = Sheet {
            identity: Identity { name: Some("Ravi Kumar".into()), code: Some("E999".into()), ..Default::default() },
            period: None,
            total: Some(18.0),
            entries: vec![entry("2026-09-01", 8.0), entry("2026-09-02", 9.5)],
        };
        let (s, issues) = sanitize_model(model, text, "Timesheet", 2026);
        assert_eq!(s.entries.len(), 1);
        assert_eq!(s.total, None);
        assert_eq!(s.identity.code, None);
        assert_eq!(s.identity.name.as_deref(), Some("Ravi Kumar"));
        assert!(issues.iter().any(|i| i.code == "model_days_dropped" && i.severity == CHECK));
    }

    #[test]
    fn a_model_day_needs_its_date_and_its_hours_on_one_line_in_any_date_format() {
        let text = "Name: Ravi\n3 Sep 2026   8\n2026-09-04 | 6\n05/09/2026 | 7.5\nSep 8, 2026: 4\n1 Sep 16\nHours: 8 on one day";
        let model = Sheet {
            entries: vec![
                entry("2026-09-03", 8.0),
                entry("2026-09-04", 6.0),
                entry("2026-09-05", 7.5),
                entry("2026-09-08", 4.0),
                entry("2026-09-01", 16.0),
                entry("2026-09-04", 8.0),
                entry("2026-09-20", 6.0),
            ],
            ..Default::default()
        };
        let kept: Vec<(String, f64)> = sanitize_model(model, text, "Timesheet", 2026).0.entries.into_iter().map(|e| (e.date.unwrap(), e.hours)).collect();
        let want: Vec<(String, f64)> =
            [("2026-09-03", 8.0), ("2026-09-04", 6.0), ("2026-09-05", 7.5), ("2026-09-08", 4.0), ("2026-09-01", 16.0)].iter().map(|(d, h)| ((*d).to_string(), *h)).collect();
        assert_eq!(kept, want, "8h on 4 Sep is on another line, 20 Sep is nowhere");
    }

    #[test]
    fn a_model_period_must_be_written_in_the_text() {
        let model = |p: (&str, &str)| Sheet { period: Some((p.0.into(), p.1.into())), ..Default::default() };
        let text = "Timesheet 1 Sep 2026 to 30 Sep 2026\n1 Sep 8";
        assert!(sanitize_model(model(("2026-09-01", "2026-09-30")), text, "x", 2026).0.period.is_some());
        assert!(sanitize_model(model(("2026-08-01", "2026-08-31")), text, "x", 2026).0.period.is_none());
        assert!(sanitize_model(model(("2026-09-01", "2026-09-30")), "Hours for the month\n1 Sep 8", "September timesheet", 2026).0.period.is_some());
        assert!(sanitize_model(model(("2026-09-01", "2026-09-30")), "1 Sep 8", "Hello", 2026).0.period.is_none(), "a month nobody named");
    }

    #[test]
    fn a_rule_total_is_never_replaced_by_the_model_total() {
        let base = Sheet { total: Some(24.0), ..Default::default() };
        let model = Sheet { total: Some(30.0), entries: vec![entry("2026-09-01", 8.0)], ..Default::default() };
        assert_eq!(merge(base, model).total, Some(24.0));
    }
}
