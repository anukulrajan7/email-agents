use super::*;

/// The shipped agent templates as a workspace with every pack on would route with them.
fn routing_agents() -> Vec<RoutingAgent> {
    TEMPLATES
        .iter()
        .filter(|t| t.scope == "workspace")
        .enumerate()
        .map(|(i, t)| {
            let spec = t.spec();
            let row = AgentRow {
                id: t.key.into(),
                workspace_id: "eval".into(),
                key: spec.key.clone(),
                kind: t.kind.into(),
                scope: t.scope.into(),
                owner_user_id: None,
                category: t.category.into(),
                template_key: Some(t.key.into()),
                template_version: Some(i64::from(t.version)),
                cloned_from: None,
                name: t.name.into(),
                avatar: t.avatar.into(),
                color: t.color.into(),
                description: spec.description.clone(),
                status: "active".into(),
                paused_reason: None,
                active_version: Some(1),
                tuned: false,
                autonomy_level: t.initial_level,
                autonomy_cap: 3,
                next_run_at: None,
                created_by: None,
                created_at: i as i64,
                updated_at: 0,
            };
            RoutingAgent { row, spec, version: 1, level: t.initial_level }
        })
        .collect()
}

/// Which agent the label would reach, by the real routing function (no database, no model).
pub fn routed_agent_name(from: &str, subject: &str, label: Option<&str>) -> String {
    let agents = routing_agents();
    let facts = MessageFacts {
        sender_address: from.into(),
        subject: subject.into(),
        label: label.map(str::to_string),
        needs_reply: true,
        ..Default::default()
    };
    route(&facts, &Hints::default(), &agents)
        .and_then(|r| agents.iter().find(|a| a.row.id == r.agent_id).map(|a| a.row.name.clone()))
        .unwrap_or_else(|| "nobody".into())
}

pub(super) fn routing_failures(s: &Scenario) -> Vec<String> {
    let mut out = Vec::new();
    let Some(want) = &s.expect.routed_agent else { return out };
    let last = s.emails.last().expect("a scenario has an email");
    let got = routed_agent_name(&last.from, &last.subject, s.label.as_deref());
    if &got != want {
        out.push(format!("routing: {got} would get this email, expected {want}"));
    }
    if let Some(x) = &s.expect.agent_spec {
        let agents = routing_agents();
        let steps: Vec<String> = agents
            .iter()
            .find(|a| &a.row.name == want && a.row.kind != "lead")
            .map(|a| a.spec.steps.iter().filter_map(|st| serde_json::to_value(st).ok()?["type"].as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        for f in x.forbid_steps.iter().filter(|f| steps.contains(f)) {
            out.push(format!("agent_spec: {want}'s template still has the {f} step"));
        }
        for r in x.require_steps.iter().filter(|r| !steps.contains(r)) {
            out.push(format!("agent_spec: {want}'s template has no {r} step"));
        }
    }
    out
}

/// The taxonomy the classifier sees for this scenario: the packs its agent works under.
pub fn label_set(s: &Scenario) -> Vec<crate::platform::domain::Label> {
    let packs: &[&str] = match s.agent() {
        "Ananya" | "Rohan" | "Sneha" | "Arjun" | "Aditya" => &["staffing"],
        "Ishaan" => &["support", "general"],
        "Kabir" => &["sales", "general"],
        "Vikram" => &["finance", "general"],
        _ => &["general", "personal"],
    };
    let mut out: Vec<crate::platform::domain::Label> = Vec::new();
    for seed in packs.iter().filter_map(|p| crate::platform::packs::pack(p)).flat_map(|p| p.labels.iter()) {
        if !out.iter().any(|l| l.key == seed.key) {
            out.push(crate::platform::domain::Label { key: seed.key.into(), name: seed.name.into(), description: seed.description.into() });
        }
    }
    out
}

/// The shipped spec of the expert a scenario belongs to, to run the scenario as that expert. `None` for the lead.
pub fn spec_for(s: &Scenario) -> Option<AgentSpec> {
    let name = s.expect.routed_agent.as_deref()?;
    TEMPLATES.iter().find(|t| t.name == name && t.kind == "system" && t.scope == "workspace").map(|t| t.spec())
}

#[derive(Debug, Clone, Default)]
pub struct AgentScore {
    pub agent: String,
    pub scenarios: usize,
    pub passed: usize,
    pub pending: usize,
    pub safety_failures: usize,
    pub avg_steps: f64,
    pub avg_cost_usd: f64,
}

impl AgentScore {
    pub fn pass_rate(&self) -> f64 {
        100.0 * self.passed as f64 / self.scenarios.max(1) as f64
    }
}

/// Pending scenarios that still fail are reported on their own and left out of the pass rate.
pub fn agent_scores(outcomes: &[Outcome]) -> Vec<AgentScore> {
    let mut by: BTreeMap<&str, Vec<&Outcome>> = BTreeMap::new();
    for o in outcomes {
        by.entry(o.agent.as_str()).or_default().push(o);
    }
    by.into_iter()
        .map(|(agent, v)| {
            let scored: Vec<&&Outcome> = v.iter().filter(|o| !o.pending_failing()).collect();
            let n = scored.len().max(1) as f64;
            AgentScore {
                agent: agent.into(),
                scenarios: scored.len(),
                passed: scored.iter().filter(|o| o.passed()).count(),
                pending: v.len() - scored.len(),
                safety_failures: v.iter().filter(|o| o.failures.iter().any(|f| f.starts_with("SAFETY"))).count(),
                avg_steps: (scored.iter().map(|o| o.steps as f64).sum::<f64>() + 0.0) / n,
                avg_cost_usd: (scored.iter().map(|o| o.case_cost_usd.max(o.total_cost_usd)).sum::<f64>() + 0.0) / n,
            }
        })
        .collect()
}

pub fn agent_scorecard(title: &str, outcomes: &[Outcome], live: bool) -> String {
    let mut out = format!("\n{title}: per agent\n{:<10} {:>9} {:>5} {:>7} {:>8} {:>10} {:>11}\n", "agent", "scenarios", "pass", "rate", "safety", "avg steps", "avg $/case");
    for a in agent_scores(outcomes) {
        let cost = if live { format!("{:.5}", a.avg_cost_usd) } else { "-".into() };
        out.push_str(&format!(
            "{:<10} {:>9} {:>5} {:>6.0}% {:>8} {:>10.1} {:>11}{}\n",
            a.agent,
            a.scenarios,
            a.passed,
            a.pass_rate(),
            a.safety_failures,
            a.avg_steps,
            cost,
            if a.pending > 0 { format!("  (+{} pending)", a.pending) } else { String::new() }
        ));
    }
    out
}

/// The per-agent table as Markdown, for `private/evals/scorecard.md` (local, git-ignored).
pub fn agent_scorecard_md(outcomes: &[Outcome], live: bool) -> String {
    let mut out = String::from("| agent | scenarios | pass | pass % | safety failures | avg steps | avg cost / case |\n|---|---:|---:|---:|---:|---:|---:|\n");
    for a in agent_scores(outcomes) {
        let cost = if live { format!("${:.5}", a.avg_cost_usd) } else { "n/a".into() };
        let pending = if a.pending > 0 { format!(" (+{} pending)", a.pending) } else { String::new() };
        out.push_str(&format!(
            "| {} | {}{} | {} | {:.0}% | {} | {:.1} | {} |\n",
            a.agent,
            a.scenarios,
            pending,
            a.passed,
            a.pass_rate(),
            a.safety_failures,
            a.avg_steps,
            cost
        ));
    }
    let scored: Vec<&Outcome> = outcomes.iter().filter(|o| !o.pending_failing()).collect();
    let pass = scored.iter().filter(|o| o.passed()).count();
    out.push_str(&format!("\nAll: {pass}/{} pass ({:.0}%).", scored.len(), 100.0 * pass as f64 / scored.len().max(1) as f64));
    if live {
        let total: f64 = outcomes.iter().map(|o| o.total_cost_usd.max(o.case_cost_usd)).sum();
        let emails: usize = outcomes.iter().map(|o| o.emails).sum();
        out.push_str(&format!(" AI spend ${total:.4} over {emails} emails (${:.5} per email).", total / emails.max(1) as f64));
    }
    out.push('\n');
    out
}

/// Per-label accuracy of a classifier: (expected, got) pairs, as text and as a Markdown table.
pub fn label_accuracy(pairs: &[(String, String)]) -> BTreeMap<String, (usize, usize)> {
    let mut by: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (want, got) in pairs {
        let e = by.entry(want.clone()).or_default();
        e.0 += 1;
        e.1 += usize::from(want == got);
    }
    by
}

pub fn label_accuracy_md(by: &BTreeMap<String, (usize, usize)>) -> String {
    let mut out = String::from("| label | emails | correct | accuracy |\n|---|---:|---:|---:|\n");
    let (n, ok) = by.values().fold((0, 0), |a, v| (a.0 + v.0, a.1 + v.1));
    for (label, (n, ok)) in by {
        out.push_str(&format!("| {label} | {n} | {ok} | {:.0}% |\n", 100.0 * *ok as f64 / (*n).max(1) as f64));
    }
    out.push_str(&format!("| all | {n} | {ok} | {:.0}% |\n", 100.0 * ok as f64 / n.max(1) as f64));
    out
}

/// Adds a run to the top of `private/evals/scorecard.md` (local, git-ignored) when `GOLDEN_WRITE_SCORECARD=1` (so CI never edits files).
/// A run with the same date, commit and kind replaces its earlier entry; older runs stay.
pub fn write_scorecard(kind: &str, body: &str) {
    if std::env::var("GOLDEN_WRITE_SCORECARD").ok().as_deref() != Some("1") {
        return;
    }
    let run = |cmd: &str, args: &[&str]| {
        std::process::Command::new(cmd)
            .args(args)
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    let heading = format!("## {} | {} | {kind}", run("date", &["+%Y-%m-%d"]), run("/usr/bin/git", &["rev-parse", "--short", "HEAD"]));
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../private/evals/scorecard.md");
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let (head, rest) = match old.find("\n## ") {
        Some(i) => (old[..i + 1].to_string(), old[i + 1..].to_string()),
        None if old.is_empty() => (
            "# Eval scorecard\n\nOne entry per run of the golden evals, newest first (`GOLDEN_WRITE_SCORECARD=1 cargo test --test golden_offline`, \
             `golden_understanding`, `golden_live`). \"Pending\" scenarios are written ahead of a change that has not merged yet and are left out of the pass rate.\n"
                .to_string(),
            String::new(),
        ),
        None => (old.clone(), String::new()),
    };
    let mut blocks: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in rest.lines() {
        if line.starts_with("## ") && !current.is_empty() {
            blocks.push(std::mem::take(&mut current));
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.is_empty() {
        blocks.push(current);
    }
    blocks.retain(|b| !b.starts_with(&format!("{heading}\n")));
    let mut text = format!("{}\n\n{heading}\n\n{}\n", head.trim_end(), body.trim_end());
    for b in blocks {
        text.push('\n');
        text.push_str(b.trim_end());
        text.push('\n');
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&path, text).expect("private/evals/scorecard.md is writable");
}
