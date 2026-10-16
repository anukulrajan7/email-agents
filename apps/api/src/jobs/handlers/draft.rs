use super::*;
use crate::replies::live_draft::{self, Mode, NewDraft};

pub(super) async fn generate_draft(state: &AppState, job: &Job, payload: &Value) -> Result<(), JobError> {
    let message_id = str_field(payload, "message_id")?;
    let run_id = payload.get("workflow_run_id").and_then(Value::as_str).map(str::to_string);
    let msg = load_message(state, &message_id).await?;
    if !job.dedupe_key.contains(":manual:") && crate::mail::system_mail::is_own_message(&state.db, &msg.workspace_id, &message_id).await? {
        return finish_without_draft(state, &msg.workspace_id, &message_id, run_id.as_deref(), "Post Room's own mail needs no reply").await;
    }
    // Automatic drafting can be switched off: then only a person's request (`:manual:`) drafts.
    let auto_reply = crate::replies::reply_settings::load(&state.db, &state.config, &msg.workspace_id).await?.auto_reply;
    if !auto_reply && !job.dedupe_key.contains(":manual:") {
        return finish_without_draft(state, &msg.workspace_id, &message_id, run_id.as_deref(), "automatic drafts are off").await;
    }
    // Timesheets are recorded by the timesheet desk, which never replies; only a person asking for a draft gets one.
    if !job.dedupe_key.contains(":manual:") && crate::hr::staffing::timesheet_records::suppresses_auto_reply(&state.db, &msg.workspace_id, &message_id).await? {
        return finish_without_draft(state, &msg.workspace_id, &message_id, run_id.as_deref(), "a timesheet was recorded; no reply is needed").await;
    }
    let insight = analyze_message(state, &json!({ "message_id": message_id })).await?;
    // Workflows don't draft replies to email that needs none (newsletters, notifications).
    if run_id.is_some() && !insight.needs_reply {
        return finish_without_draft(state, &msg.workspace_id, &message_id, run_id.as_deref(), "no reply needed").await;
    }
    let requested = (payload.get("quality").and_then(Value::as_str) == Some("premium")).then_some(ai::DraftQuality::Premium);
    let by_person = job.dedupe_key.contains(":manual:");
    // A person asking for a draft rewrites the open one; an automatic run keeps whatever is already there (an automation's draft too).
    let mode = if by_person { Mode::Replace { allow_person_edited: true } } else { Mode::KeepExisting };
    let generated = match live_draft::settled(&state.db, &message_id, mode).await? {
        Some(_) => None,
        None => Some(compose_draft(state, &message_id, &msg, insight, payload.get("instructions").and_then(Value::as_str), requested, !by_person, None).await?),
    };
    if let Some((out, _, _)) = &generated
        && out.body_text.trim().is_empty()
    {
        return finish_without_draft(state, &msg.workspace_id, &message_id, run_id.as_deref(), "model judged no reply needed").await;
    }

    let now = now_ms();
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let mut written = None;
    if let Some((out, ctx, quality)) = &generated {
        let w = live_draft::write(
            &mut tx,
            mode,
            &NewDraft {
                workspace_id: &msg.workspace_id,
                message_id: &message_id,
                origin_key: &job.dedupe_key,
                body_text: &out.body_text,
                model_id: &out.model_id,
                memory_ids_json: json!(ctx.ids()).to_string(),
                unanswered_json: json!(out.unanswered).to_string(),
                audit: json!({ "model_id": out.model_id, "prompt_version": ai::DRAFT_PROMPT_VERSION, "memories_used": ctx.ids().len(), "tier": quality.as_str() }),
            },
        )
        .await?;
        if let Some(draft_id) = w.changed() {
            let author = payload.get("owner_user_id").and_then(Value::as_str);
            let words: Vec<&str> = [payload.get("owner_said"), payload.get("instructions")].into_iter().flatten().filter_map(Value::as_str).collect();
            if words.is_empty() {
                crate::replies::owner_said::record(&mut tx, &msg.workspace_id, draft_id, &message_id, author, None).await?;
            }
            for w in words {
                crate::replies::owner_said::record(&mut tx, &msg.workspace_id, draft_id, &message_id, author, Some(w)).await?;
            }
            crate::learning::memory_learning::record_use(&mut tx, &ctx.ids(), now).await?;
            learning::preferences::record_use(&mut tx, &ctx.preference_ids).await?;
        } else if let live_draft::Written::Busy(busy) = &w {
            audit::record(
                &mut tx,
                Entry {
                    workspace_id: &msg.workspace_id,
                    actor: Actor::System,
                    action: "draft.skipped",
                    entity_type: "draft",
                    entity_id: &busy.id,
                    message_id: Some(&message_id),
                    metadata: json!({ "reason": "a send is scheduled for the current draft" }),
                },
            )
            .await?;
        }
        written = Some(w);
    }
    if let Some(run_id) = &run_id {
        let completed = sqlx::query("UPDATE workflow_runs SET status = 'completed', error = NULL WHERE id = ?1 AND status != 'completed'")
            .bind(run_id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            == 1;
        sqlx::query("UPDATE email_messages SET processing_status = 'workflow_completed', processing_error = NULL WHERE id = ?1")
            .bind(&message_id)
            .execute(&mut *tx)
            .await?;
        if completed {
            audit::record(
                &mut tx,
                Entry {
                    workspace_id: &msg.workspace_id,
                    actor: Actor::System,
                    action: "workflow.completed",
                    entity_type: "workflow_run",
                    entity_id: run_id,
                    message_id: Some(&message_id),
                    metadata: json!({}),
                },
            )
            .await?;
        }
    }
    tx.commit().await?;
    if let Some(w) = &written {
        live_draft::settle(state, w).await;
    }
    Ok(())
}

/// Runs the drafter with the workspace's labels, trusted memory and the email's insight.
/// `extra_instructions` (from an automation) is passed to the drafter as one more memory note.
/// `automated` (nobody asked for this draft): past replies only from the sender's own domain and
/// no auto-learned workspace-wide notes, so outside mail can't pull other clients' data into it.
#[allow(clippy::too_many_arguments)]
async fn compose_draft(
    state: &AppState,
    message_id: &str,
    msg: &MessageForAi,
    insight: ai::Insight,
    extra_instructions: Option<&str>,
    requested: Option<ai::DraftQuality>,
    automated: bool,
    agent_id: Option<&str>,
) -> Result<(ai::DraftOutput, memory::MemoryContext, ai::DraftQuality), JobError> {
    let classification: Option<(String, String)> =
        sqlx::query_as("SELECT category, priority FROM classifications WHERE message_id = ?1")
            .bind(message_id)
            .fetch_optional(&state.db)
            .await?;
    let (category, priority) = classification.unwrap_or_else(|| (OTHER_LABEL.to_string(), Priority::Normal.as_str().to_string()));
    let (category_name, category_description) = labels::active(&state.db, &msg.workspace_id)
        .await?
        .into_iter()
        .find(|l| l.key == category)
        .map(|l| (l.name, l.description))
        .unwrap_or_else(|| (category.clone(), String::new()));
    let staffing = crate::platform::packs::enabled(&state.db, &msg.workspace_id, crate::platform::packs::STAFFING).await?;
    // A sender the mailbox owner marked as important gets the premium tier.
    let vip = crate::agents::profiles::context_for_message(&state.db, &state.config, &msg.workspace_id, message_id)
        .await?
        .owner
        .and_then(|o| o.profile)
        .is_some_and(|p| ai::routing::is_vip(&p.vips, &msg.sender_address));
    let quality = ai::routing::draft_quality(
        Priority::parse(&priority).unwrap_or(Priority::Normal),
        &category,
        Some(&insight),
        requested,
        staffing,
        vip,
    );
    let settings = crate::replies::reply_settings::load(&state.db, &state.config, &msg.workspace_id).await?;
    let reply_instructions = if settings.reply_instructions.is_empty() {
        crate::platform::packs::reply_style(&state.db, &msg.workspace_id).await?.to_string()
    } else {
        settings.reply_instructions
    };
    let default_sign_off = crate::platform::packs::default_sign_off(&state.db, &msg.workspace_id).await?;
    let (reply_instructions, signature) =
        crate::agents::profiles::draft_voice(&state.db, &state.config, &msg.workspace_id, message_id, reply_instructions, settings.signature).await?;

    // Trusted memory (staff notes + learned sender profile), retrieved by sender and topic, within the agent's memory scope.
    let agent = {
        let mut conn = state.db.acquire().await?;
        learning::scope::resolve(&mut conn, &msg.workspace_id, agent_id, Some(message_id)).await?
    };
    let view = agent.agent_id.clone().map(|id| memory::AgentMemory { agent_id: id, scope: agent.memory });
    let mut ctx = memory::context_for_agent(&state.db, &msg.workspace_id, &msg.sender_address, &msg.subject, automated, view.as_ref()).await?;
    let preferences = learning::retrieve::for_draft(
        &state.db,
        state.embedder.as_ref(),
        &msg.workspace_id,
        agent.agent_id.as_deref(),
        agent.owner_user_id.as_deref(),
        &format!("{}\n{}", msg.subject, msg.body_text),
    )
    .await?;
    ctx.preference_ids = preferences.iter().map(|p| p.id.clone()).collect();
    let mut memory_notes = ctx.lines();
    let team_notes = memory_notes.clone();
    let extra_instructions = extra_instructions.map(crate::replies::signoff::strip_sign_off_instruction);
    if let Some(extra) = extra_instructions.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
        memory_notes.push(format!("Instruction for this reply: {extra}"));
    }
    let (db, embedder) = (&state.db, state.embedder.as_ref());
    let examples = if automated {
        let domain = msg.sender_address.rsplit_once('@').map(|(_, d)| d.to_lowercase()).unwrap_or_default();
        crate::learning::knowledge::reply_examples_from_domain(db, embedder, &msg.workspace_id, message_id, &msg.subject, &msg.body_text, &domain).await
    } else {
        crate::learning::knowledge::reply_examples(db, embedder, &msg.workspace_id, message_id, &msg.subject, &msg.body_text).await
    };
    let input = DraftInput {
        examples,
        category,
        category_name,
        category_description,
        subject: msg.subject.clone(),
        sender: msg.sender_address.clone(),
        body_text: ai::body::model_body(&msg.subject, &msg.body_text),
        memory_notes,
        writer_preferences: learning::retrieve::lines(&preferences),
        sender_history: ctx.sender_history.clone(),
        insight: Some(insight),
        quality,
        reply_instructions,
        signature,
        default_sign_off,
        thread_digest: thread_digest(state, message_id).await?,
    };
    let mut out = state.drafter.draft(&input).await?;
    {
        // Nobody signs as an assistant: any of the workspace's agent names as the closing line becomes the owner's sign-off.
        let names: Vec<String> = sqlx::query_scalar("SELECT DISTINCT name FROM agents WHERE workspace_id = ?1").bind(&msg.workspace_id).fetch_all(&state.db).await?;
        let own = input.signature.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or(&input.default_sign_off);
        for name in names {
            if crate::replies::signoff::replace_agent_sign_off(&mut out.body_text, &name, own) {
                tracing::info!(message_id, "an agent's own name at the end of a draft was replaced with the owner's sign-off");
                break;
            }
        }
    }
    // The critic: anything the verified context pack doesn't support becomes a [NEEDS: ...] for a person.
    if !out.body_text.trim().is_empty() {
        let pack = learning::context_pack::ContextPack::from_input(&input);
        let unsupported = learning::context_pack::check_draft(&pack, &mut out);
        if !unsupported.is_empty() {
            tracing::info!(message_id, claims = unsupported.len(), "draft claims not in the context pack were turned into [NEEDS]");
            crate::ai::incidents::record(
                &state.db,
                crate::ai::incidents::NewIncident::new(&msg.workspace_id, crate::ai::incidents::Kind::UnsupportedClaim, json!({"claims": unsupported.len()}))
                    .message(Some(message_id))
                    .dedupe(format!("unsupported:{message_id}")),
            )
            .await;
        }
    }
    if !out.body_text.trim().is_empty() {
        let sources = crate::replies::gap_resolver::Sources {
            ws: &msg.workspace_id,
            staffing,
            subject: &msg.subject,
            email_body: &msg.body_text,
            category: &input.category,
            notes: &team_notes,
        };
        crate::replies::gap_resolver::resolve_draft(&state.db, &sources, &mut out).await?;
    }
    Ok((out, ctx, quality))
}

/// Drafts a reply for automations. Reuses the message's live draft (or an already sent reply) if there is one.
/// `Ok(None)` means the email needs no reply (or the model declined to write one).
pub async fn create_draft_for_message(
    state: &AppState,
    message_id: &str,
    extra_instructions: Option<&str>,
) -> Result<Option<String>, JobError> {
    if let Some(w) = live_draft::settled(&state.db, message_id, Mode::KeepExisting).await? {
        return Ok(Some(w.id().to_string()));
    }
    let msg = load_message(state, message_id).await?;
    if crate::mail::system_mail::is_own_message(&state.db, &msg.workspace_id, message_id).await? {
        return Ok(None);
    }
    if crate::hr::staffing::timesheet_records::suppresses_auto_reply(&state.db, &msg.workspace_id, message_id).await? {
        return Ok(None);
    }
    let insight = analyze_message(state, &json!({ "message_id": message_id })).await?;
    if !insight.needs_reply {
        return Ok(None);
    }
    let (out, ctx, quality) = compose_draft(state, message_id, &msg, insight, extra_instructions, None, true, None).await?;
    if out.body_text.trim().is_empty() {
        return Ok(None);
    }
    let origin_key = format!("automation-draft:{message_id}");
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let written = live_draft::write(
        &mut tx,
        Mode::KeepExisting,
        &NewDraft {
            workspace_id: &msg.workspace_id,
            message_id,
            origin_key: &origin_key,
            body_text: &out.body_text,
            model_id: &out.model_id,
            memory_ids_json: json!(ctx.ids()).to_string(),
            unanswered_json: json!(out.unanswered).to_string(),
            audit: json!({ "model_id": out.model_id, "prompt_version": ai::DRAFT_PROMPT_VERSION, "memories_used": ctx.ids().len(), "by": "automation", "tier": quality.as_str() }),
        },
    )
    .await?;
    tx.commit().await?;
    live_draft::settle(state, &written).await;
    Ok(Some(written.id().to_string()))
}

/// Drafts a reply for an agent step. One draft per `origin_key` (re-runs reuse it) and one live draft per
/// message: without instructions an existing live draft is reused, with instructions a suggested one is rewritten
/// (a draft a person edited, approved or scheduled is never touched). The agent picks the instructions and tier.
/// `Ok(None)` means the writer judged that no reply is needed.
pub async fn create_agent_draft(
    state: &AppState,
    message_id: &str,
    instructions: Option<&str>,
    requested: Option<ai::DraftQuality>,
    origin_key: &str,
    agent_id: Option<&str>,
    owner: Option<(&str, &str)>,
) -> Result<Option<(String, String)>, JobError> {
    let existing: Option<(String, String)> = sqlx::query_as("SELECT id, body_text FROM drafts WHERE origin_key = ?1")
        .bind(origin_key)
        .fetch_optional(&state.db)
        .await?;
    if existing.is_some() {
        return Ok(existing);
    }
    let mode = if instructions.is_some() { Mode::Replace { allow_person_edited: false } } else { Mode::KeepExisting };
    match live_draft::settled(&state.db, message_id, mode).await? {
        Some(live_draft::Written::Existing(d) | live_draft::Written::Busy(d)) => return Ok(Some((d.id, d.body_text))),
        Some(_) => return Ok(None),
        None => {}
    }
    let msg = load_message(state, message_id).await?;
    if crate::hr::staffing::timesheet_records::suppresses_auto_reply(&state.db, &msg.workspace_id, message_id).await? {
        return Ok(None);
    }
    let insight = analyze_message(state, &json!({ "message_id": message_id })).await?;
    let (out, ctx, quality) = compose_draft(state, message_id, &msg, insight, instructions, requested, true, agent_id).await?;
    if out.body_text.trim().is_empty() {
        return Ok(None);
    }
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    let written = live_draft::write(
        &mut tx,
        mode,
        &NewDraft {
            workspace_id: &msg.workspace_id,
            message_id,
            origin_key,
            body_text: &out.body_text,
            model_id: &out.model_id,
            memory_ids_json: json!(ctx.ids()).to_string(),
            unanswered_json: json!(out.unanswered).to_string(),
            audit: json!({ "model_id": out.model_id, "prompt_version": ai::DRAFT_PROMPT_VERSION, "memories_used": ctx.ids().len(), "by": "agent", "tier": quality.as_str() }),
        },
    )
    .await?;
    if let Some(draft_id) = written.changed() {
        crate::replies::owner_said::record(&mut tx, &msg.workspace_id, draft_id, message_id, owner.map(|o| o.0), owner.map(|o| o.1)).await?;
    }
    tx.commit().await?;
    live_draft::settle(state, &written).await;
    Ok(match written {
        live_draft::Written::Inserted { id, .. } | live_draft::Written::Replaced { id, .. } => Some((id, out.body_text)),
        live_draft::Written::Existing(d) | live_draft::Written::Busy(d) => Some((d.id, d.body_text)),
        live_draft::Written::AlreadySent(_) => None,
    })
}

/// Writes a reply without storing it, so people can try out the agent's instructions.
pub async fn preview_draft(state: &AppState, message_id: &str, requested: Option<ai::DraftQuality>) -> Result<String, JobError> {
    let msg = load_message(state, message_id).await?;
    let insight = analyze_message(state, &json!({ "message_id": message_id })).await?;
    let (out, _, _) = compose_draft(state, message_id, &msg, insight, None, requested, false, None).await?;
    Ok(out.body_text)
}

async fn finish_without_draft(
    state: &AppState,
    workspace_id: &str,
    message_id: &str,
    run_id: Option<&str>,
    reason: &str,
) -> Result<(), JobError> {
    let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
    if let Some(run_id) = run_id {
        sqlx::query("UPDATE workflow_runs SET status = 'completed', error = NULL WHERE id = ?1").bind(run_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE email_messages SET processing_status = 'workflow_completed', processing_error = NULL WHERE id = ?1")
            .bind(message_id)
            .execute(&mut *tx)
            .await?;
    }
    audit::record(
        &mut tx,
        Entry {
            workspace_id,
            actor: Actor::System,
            action: "draft.skipped",
            entity_type: "message",
            entity_id: message_id,
            message_id: Some(message_id),
            metadata: json!({ "reason": reason }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
