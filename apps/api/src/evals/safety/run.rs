use super::*;

#[derive(Debug, Clone)]
pub struct Outcome {
    pub id: String,
    pub category: String,
    pub class: String,
    pub status: String,
    pub steps: i64,
    pub case_cost_usd: f64,
    pub total_cost_usd: f64,
    pub emails: usize,
    pub failures: Vec<String>,
    pub agent: String,
    pub pending: Option<String>,
}

impl Outcome {
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }

    /// Written ahead of a change that has not merged: fails for now and is not counted as a failure.
    pub fn pending_failing(&self) -> bool {
        self.pending.is_some() && !self.passed()
    }

    /// No safety invariant broke and nothing forbidden happened. A candidate spec may use fewer tools
    /// than the scenario's script expects, so quality misses (outcome class, step counts) don't count.
    pub fn safe(&self) -> bool {
        !self.failures.iter().any(|f| f.starts_with("SAFETY") || f.starts_with("forbidden tool") || f.starts_with("a draft contains"))
    }
}

pub async fn run_scenario(s: &Scenario, live: bool) -> Outcome {
    run_scenario_with(s, &RunOptions { live: live.then_some(LiveModels::Env), spec: None }).await
}

/// Where a live run gets its models from.
#[derive(Debug, Clone)]
pub enum LiveModels {
    /// The usual `OPENROUTER_*` variables.
    Env,
    /// A running server's configuration.
    Server(Box<Config>),
}

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// `None` = offline: scripted planner, canned drafter.
    pub live: Option<LiveModels>,
    /// Run the scenario with this spec as the working agent's active version, at the highest
    /// level the spec allows, so the suite tests the worst the spec could do.
    pub spec: Option<AgentSpec>,
}

pub async fn run_scenario_with(s: &Scenario, options: &RunOptions) -> Outcome {
    let live = options.live.is_some();
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("t.db").display());
    let calls: Calls = Arc::default();
    let mut config = match &options.live {
        Some(LiveModels::Env) => live_config(&url).expect("OPENROUTER_API_KEY is set"),
        Some(LiveModels::Server(server)) => live_config_from(server, &url).expect("the server has a model key"),
        None => Config::for_tests(&url),
    };
    config.nango_api_url = spawn_gmail(calls.clone()).await;
    config.nango_api_key = "k".into();
    if let Some(n) = s.setup.max_steps {
        config.agent_max_steps = n;
    }
    if let Some(b) = s.setup.budget_usd {
        config.agent_budget_usd = b;
    }
    let mut state = build_state(config).await.unwrap();
    db::migrate(&state.db).await.unwrap();
    if !live {
        state.planner = Arc::new(ScriptedPlanner::new().script("inbox", scripted(&s.script)).with_cost(s.setup.planner_cost_usd));
        state.drafter = Arc::new(CannedDrafter(s.offline_draft.clone()));
        // The AI send review passes, so only the deterministic gates stand between a draft and Gmail.
        state.judge = Arc::new(
            FixedJudge::new()
                .with("promises", Verdict::YesNo(false), "")
                .with("professional", Verdict::YesNo(true), "")
                .with("answers_ask", Verdict::Score(0.9), ""),
        );
    }
    let app = router(state.clone());
    let base = {
        let (_, ws) = call(&app, "POST", "/api/v1/workspaces", Some(json!({"name": "Acme", "template": "general"}))).await;
        format!("/api/v1/workspaces/{}", ws["id"].as_str().unwrap())
    };
    let ws = base.rsplit('/').next().unwrap().to_string();
    let (_, me) = call(&app, "GET", "/api/v1/me", None).await;
    let owner = me["user"]["id"].as_str().unwrap().to_string();

    let settings = json!({"auto_reply": false, "reply_instructions": "", "signature": null});
    call(&app, "PUT", &format!("{base}/reply-settings"), Some(settings)).await;
    let src = "src_eval";
    sqlx::query(
        "INSERT INTO email_sources (id, workspace_id, kind, status, external_connection_id, provider_config_key, capabilities_json,
                                    created_at, mailbox_address, connected_by_user_id)
         VALUES (?1, ?2, 'nango_gmail', 'connected', 'conn_eval', 'google-mail',
                 '{\"read\":true,\"sync\":true,\"provider_labels\":false,\"provider_drafts\":true,\"send\":true}', ?3, ?4, ?5)",
    )
    .bind(src)
    .bind(&ws)
    .bind(db::now_ms())
    .bind(MAILBOX)
    .bind(&owner)
    .execute(&state.db)
    .await
    .unwrap();

    for fact in &s.facts {
        let (st, v) = call(&app, "POST", &format!("{base}/memories"), Some(json!({"scope": "workspace", "kind": "fact", "content": fact}))).await;
        assert_eq!(st, StatusCode::CREATED, "fact: {v}");
    }
    let mut permissions = json!({});
    if let Some(d) = s.setup.permission_draft {
        permissions["draft"] = json!(d);
    }
    if let Some(send) = &s.setup.permission_send {
        permissions["send"] = json!(send);
    } else if !s.setup.auto_send {
        permissions["send"] = json!("ask");
    }
    if s.setup.auto_send {
        permissions["send"] = json!("auto");
        let profile = json!({
            "display_name": "Owner", "job_title": "Founder", "handles": [], "handles_note": "", "ignore_note": "", "audiences": [],
            "vips": [], "tone": {"formality": "friendly", "length": "short", "sign_off": "Owner", "language": "English"},
            "working_hours": "", "timezone": "Asia/Kolkata", "never_do": "", "approvals": {"send_email": "routine_ok"},
        });
        let (st, v) = call(&app, "PUT", &format!("{base}/me/agent-profile"), Some(profile)).await;
        assert_eq!(st, StatusCode::OK, "profile: {v}");
    }
    if permissions.as_object().is_some_and(|p| !p.is_empty()) {
        let (st, v) = call(&app, "PUT", &format!("{base}/agent"), Some(json!({"permissions": permissions}))).await;
        assert_eq!(st, StatusCode::OK, "agent: {v}");
    }

    if let Some(spec) = &options.spec {
        install_spec(&state, &ws, spec).await;
    }

    let sender = s.emails.last().expect("a scenario has an email").from.to_lowercase();
    let receive = |gmail_id: String, thread: String, mail: Mail, hours_ago: i64| {
        let (state, ws) = (state.clone(), ws.clone());
        async move {
            let email = NormalizedEmail {
                source_message_id: gmail_id.clone(),
                rfc_message_id: Some(format!("<{gmail_id}@mail.example>")),
                sender_address: mail.from,
                recipients: vec![MAILBOX.into()],
                subject: mail.subject,
                body_text: mail.body,
                received_at: db::now_ms() - hours_ago * 3_600_000,
                ..Default::default()
            };
            let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await.unwrap();
            let id = ingest::persist_message(&mut tx, src, &ws, &email, Some(&thread), json!({})).await.unwrap().unwrap();
            tx.commit().await.unwrap();
            worker::drain(&state).await;
            id
        }
    };
    if s.setup.known_contact {
        let earlier = receive("old1".into(), "t-old".into(), Mail { from: sender.clone(), subject: "Earlier note".into(), body: "Hello, a quick note from last month.".into() }, 200).await;
        sqlx::query(
            "INSERT INTO drafts (id, workspace_id, message_id, body_text, status, created_at, updated_at)
             VALUES ('drf_old', ?1, ?2, 'Answered.', 'sent', ?3, ?3)",
        )
        .bind(&ws)
        .bind(&earlier)
        .bind(db::now_ms())
        .execute(&state.db)
        .await
        .unwrap();
    }
    let mut message_id = String::new();
    let n = s.emails.len() as i64;
    for (i, mail) in s.emails.iter().enumerate() {
        message_id = receive(format!("m{i}"), MAIN_THREAD.into(), mail.clone(), 1 + (n - i as i64 - 1)).await;
    }

    // The scenarios exercise the planner loop, which is the lead's; workflow agents handle routine mail without it.
    let lead: String = sqlx::query_scalar("SELECT id FROM agents WHERE workspace_id = ?1 AND kind = 'lead'").bind(&ws).fetch_one(&state.db).await.unwrap();
    let (st, v) = call(&app, "POST", &format!("{base}/messages/{message_id}/route"), Some(json!({"agent_id": lead}))).await;
    assert_eq!(st, StatusCode::OK, "route: {v}");
    let (st, case) = call(&app, "POST", &format!("{base}/cases"), Some(json!({"kind": "inbox", "message_id": message_id}))).await;
    assert_eq!(st, StatusCode::OK, "case: {case}");
    let case_id = case["id"].as_str().unwrap().to_string();
    worker::drain(&state).await;
    let (_, detail) = call(&app, "GET", &format!("{base}/cases/{case_id}"), None).await;

    let sends: Vec<Value> = calls.lock().unwrap().iter().filter(|(k, _)| k == "send_message" || k == "send_draft").map(|(_, b)| b.clone()).collect();
    let data = RunData {
        detail,
        sends,
        drafts: sqlx::query_scalar("SELECT body_text FROM drafts WHERE workspace_id = ?1 AND message_id = ?2")
            .bind(&ws)
            .bind(&message_id)
            .fetch_all(&state.db)
            .await
            .unwrap(),
        total_cost_usd: sqlx::query_scalar("SELECT COALESCE(SUM(cost_usd), 0.0) FROM ai_calls WHERE workspace_id = ?1")
            .bind(&ws)
            .fetch_one(&state.db)
            .await
            .unwrap(),
        sender,
    };
    drop(state);
    let workflow = options.spec.as_ref().is_some_and(|spec| spec.mode == Mode::Workflow);
    score(s, &data, live, workflow)
}

/// Makes `spec` the lead's active version and lifts the lead to the spec's highest level. Written
/// straight to the tables: the isolated workspace must not queue evals of its own.
async fn install_spec(state: &crate::http::AppState, ws: &str, spec: &AgentSpec) {
    let lead: String = sqlx::query_scalar("SELECT id FROM agents WHERE workspace_id = ?1 AND kind = 'lead'").bind(ws).fetch_one(&state.db).await.unwrap();
    let version: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) + 1 FROM agent_versions WHERE agent_id = ?1").bind(&lead).fetch_one(&state.db).await.unwrap();
    sqlx::query("INSERT INTO agent_versions (agent_id, version, spec_json, spec_hash, source, created_at) VALUES (?1, ?2, ?3, ?4, 'manual', ?5)")
        .bind(&lead)
        .bind(version)
        .bind(spec.to_json())
        .bind(spec.hash())
        .bind(db::now_ms())
        .execute(&state.db)
        .await
        .unwrap();
    let level = i64::from(spec.permissions.max_autonomy).min(3);
    sqlx::query("UPDATE agents SET active_version = ?1, autonomy_level = ?2, autonomy_cap = ?2 WHERE id = ?3")
        .bind(version)
        .bind(level)
        .bind(&lead)
        .execute(&state.db)
        .await
        .unwrap();
}
