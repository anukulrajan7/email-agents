# Shrikant requirements: verification audit (14 Oct 2026)

Checked at `dbf45f8` on branch `wp-verify` (main plus Packs M1). Method: read code and migrations, then ran the tests that
claim to cover each item. Strict rule: code with no test and no UI is **Partial**; nothing at all is **Missing**.

Tests run (all green):
- API: `cargo test --test agent_profiles --test agents --test alerts --test assistant --test attachments --test gmail_backfill --test inbox_insights --test knowledge --test matching --test matching_semantic --test packs --test pipeline` (84 tests, 0 failed).
- Web: `vitest run src/features/{alerts,matching,assistant,inbox,agents,packs}` (15 files, 96 tests, 0 failed).
- Not run: golden/live-LLM evals (`tests/golden/` does not exist in this tree; the offline stubs are the only AI in tests, so no test proves real model quality).

## 1. Verdict table

| Req | Client ask | Verdict | One-line reason |
|---|---|---|---|
| R1 | Individual performance widget: today, this week, target vs achieved | **Missing** | No people, targets, submission counts, metrics endpoint or widget exist anywhere in code or UI. |
| R2 | Billable vs non-billable employees; billable = important mail + monthly timesheets only | **Missing** | No employee type, no client-email-ID concept, no mail filtering by type. Only a generic `timesheet` label exists. |
| R3 | Match email data with portal data; reminders/approvals escalate by level | **Partial** (alerts) / **Missing** (portal match) | Alert escalation engine is built and tested, but it is one workspace-wide chain, and there is no portal data or comparison at all. |
| R4 | Read all emails and attachments first, produce outputs later | **Partial** | Attachments of every kind become text (tested). But default backfill is only 14 days / 100 mails per mailbox, and AI is deliberately skipped for mail older than 2 h (`AI_AUTO_HOURS`) until someone opens it. |
| R5 | Agent reads all team members' company email | **Partial** | Several Gmail mailboxes can be connected (admin-only), all land in one shared inbox, and the agent knows the mailbox owner. No people directory, no per-person scoping or consent flow, no test of two real mailboxes. |
| R6 | Agent answers questions on all data; monitors submissions + quality since joining | **Partial** (Q&A) / **Missing** (monitoring) | Ask Meera answers cited questions from mail search (tested). It has no counts/metric tools, no submissions data, no joining date, no quality metric. |
| R7 | Daily referral profiles matched against client requirements (Capgemini, TCS) | **Working** (matching) / **Partial** (daily) | Explainable matching, synonyms, vector recall and a once-a-day digest email are built and tested end to end. Gaps below: only two fixed labels feed it, 5-attachment cap, no UI for the digest, LLM rerank not built. |

## 2. Evidence per requirement

### R1 Performance widget: Missing
- Searched `apps/api/src`, `migrations`, `apps/web/src` for people/performance/target/billable/joining: no hits beyond unrelated words. No migration has a `people`, `targets` or `submissions` table. `nav.ts` has no dashboard; no corner widget in `AppShell.tsx`.
- Closest thing: `submit_candidate` agent tool (`agents/tools.rs:824`) and `cases` rows, tested in `agents.rs::requirement_case_runs_to_an_interview`. That is an approval-gated agent action, not a count of a recruiter's submissions, and it ignores submissions that recruiters send by hand from Gmail.
- A user can do today: nothing for this. They can only open Agents and read cases one by one.
- Gap: people directory (joining date, role, daily/weekly target), submission detection from sent mail with CV attached, outcome tracking, metric endpoint, corner widget.

### R2 Billable / non-billable: Missing
- No `billable` or employee-type concept in code. Mailbox ownership exists only as `email_sources.connected_by_user_id` (`migrations/0002`), which links a mailbox to a login user, not to an employee with a type or client.
- Nothing recognises "client email ID" or filters which mail reaches the HR/timesheet mailbox. Billable staff would simply connect Gmail like anyone and all their client mail would be read.
- Partial building blocks: `timesheet` label and `timesheet_review` workflow (`domain.rs:63,171`), asserted by `pipeline.rs::timesheet_vertical_slice_end_to_end`; preset `acknowledge_timesheets` (`automations.rs:335`). These treat timesheet as one label for everyone.
- User can do today: label/auto-acknowledge timesheet mail and open a review item. They cannot say who is billable, nor expect monthly timesheet or missing-timesheet tracking.
- Gap: employee type + client mapping, mailbox policy ("billable: only important + timesheet leave the person's mailbox"), month-end timesheet-missing alert.

### R3 Reconcile with portal; escalate by layer: Partial
Built and tested (alerts):
- `alerts.rs` rules `important_unanswered` and `approval_pending` (`alerts.rs:33-34`, `evaluate_workspace` at `:449`), 15-minute job, dedupe, snooze, auto-resolve when the team replies, daily digest, email delivery.
- API `GET /workspaces/{ws}/alerts`, `PATCH .../alerts/{id}`, `GET/PUT .../alert-settings`, `POST .../alert-settings/test` (`http/mod.rs:148-151`). UI: Alerts page, settings panel, nav badge (`features/alerts/*`).
- Tests: `alerts.rs::rules_raise_batch_escalate_and_auto_resolve` (1 h too fresh, 5 h raises, +25 h escalates to boss, reply and review close auto-resolve), `patch_snooze_settings_validation_and_test_email`, `raise_is_idempotent_and_digest_is_daily`; web `alerts.test.tsx` (resolve, snooze, reopen, settings, test send). All pass.
- Steps a user can do today: Alerts, Settings: set recipients and an "escalate to" list, set hours; important or unanswered mail raises an alert, emails the recipients, and after 24 h escalates to the second list.

Gaps (strict):
- Escalation is one flat, workspace-wide list (`recipients` then `escalate_to`, `alerts.rs:606`). Levels 2 and 3 go to the same people. There is no "respective layer": no employee, manager, HR chain, and the first notified person is not the mailbox owner. Roadmap section 2 promised owner, manager, HR.
- No portal data exists: no CSV import, no portal tables, no comparison, no "mismatch" alert kind. `grep reconcil` only hits an unrelated send-safety word. This half of R3 is entirely absent.
- No "timesheet missing", "below target", or "correction needed" alert kinds. "Corrections" are not modelled.

### R4 Read everything first, outputs later: Partial
Working and tested:
- Attachment reading for all mails/labels: PDF, DOCX, XLSX/CSV, PPTX, images (OCR via vision), audio (transcription), `attachments/mod.rs`, job `extract_attachments` (`jobs/attachment_jobs.rs`). Test: `attachments.rs::every_attachment_becomes_text_that_search_insight_and_the_ui_can_use`, `old_mail_reads_documents_now_and_ai_files_when_opened`, `a_spent_budget_skips_ai_files_but_not_documents`, `provider_errors_are_recorded...`. UI: attachment chips with read status (`AttachmentChips.test.tsx`).
- Mail is stored and searchable (`knowledge.rs::indexes_mail_and_finds_it_by_keyword_and_meaning`), insights per mail (`inbox_insights.rs`), backfill retry (`gmail_backfill.rs::backfills_full_messages_once_with_retry`).
- "Outputs later" works by design: labelling is cheap and immediate, drafts/agents run later and need approval.

Gaps:
- "Read all" is bounded: `NANGO_BACKFILL_DAYS=14`, `NANGO_BACKFILL_MAX=100` by default (`config.rs:124-125`), so a mailbox's history older than 2 weeks is never read. Test config uses 500.
- AI stages (insight, extraction, embeddings, attachment AI) are skipped automatically for mail older than `AI_AUTO_HOURS` (default 2, `config.rs:126`; `handlers.rs:51 deferred_until_opened`) and also when the daily budget is spent. So most imported history is stored but not understood until a human opens each mail. That contradicts "read all emails first".
- Per-attachment text capped at 20k chars stored, 3k each / 8k total into prompts (`attachments/mod.rs:19-24`); `MAX_PER_MESSAGE=20`.
- Matching extraction still reads at most 5 attachments (`matching_jobs.rs`, `LIMIT 5`).
- No UI shows "N of M mails read" for the workspace (only per-attachment status).

### R5 Agent reads all team members' mail: Partial
- Connecting: `POST /workspaces/{ws}/sources/nango-session` and `nango-connections` (Admin only, `http/sources.rs:171,200`), so an admin must connect each mailbox, or each person must be made admin. Not a per-person consent flow.
- Many Gmail sources per workspace are supported (`UNIQUE(kind, external_connection_id)`); `connected_by_user_id` identifies the owner, used in `alerts.rs:276`, `send_safety.rs:339`, `memory_learning.rs:340`, and per-case `agent_user_id` (`migrations/0026`, `agents/triggers.rs:91`) so the agent adapts to the mailbox owner's profile. Profiles tested in `agent_profiles.rs` (8 tests) and `agentProfiles.test.tsx`.
- Gaps: no test connects two mailboxes and checks isolation; inbox has no per-mailbox/per-person filter in the list API (`http/messages.rs`); no people directory; admin-only connect; everyone with workspace access sees all mail (no scoping). Wave3 item 5 (people + per-person mailboxes) is not built.

### R6 Agent answers questions and monitors performance: Partial / Missing
- Working: `POST /workspaces/{ws}/assistant/command` (Ask Meera, Ctrl+K, voice or text, cited answers, actions only after Confirm). Tools: `search_mail`, `list_inbox`, `summarize_email`, `list_approvals`, `list_alerts`, `get_case`, `list_matches`, plus action cards (`assistant/command.rs`, tool_specs). Tests: `assistant.rs::read_only_query_is_answered_with_citations_and_the_planner_never_sees_bodies`, `inbox_filters_return_typed_summaries`, `the_loop_is_bounded_to_six_turns`, 13 tests green; web `assistant.test.tsx` 9 tests green.
- User can ask: "Which mail needs a reply?", "What did Capgemini send about Java?", "What is waiting for my approval?".
- Gaps: no exact-count/SQL tool, so "How many submissions did Priya do since joining" cannot be answered (an LLM over search results would guess). No submissions or outcomes model, no joining date, no quality (interview rate) metric, no per-person view, no scheduled performance summary. Answers are capped at 120 words and 8 search hits. The "monitor" half of R6 is Missing.

### R7 Referral to requirement matching: Working, with gaps
- Pipeline: label `candidate_referral` / `client_requirement` -> extraction (LLM + attachments) -> skill taxonomy + vector recall -> explainable score (skills 50, experience 20, location 15, semantic fit) -> matches -> `match_digest` daily email (`jobs/matching_jobs.rs`, `jobs/match_digest.rs`, `matching.rs`, `matching_semantic`).
- API `GET /workspaces/{ws}/matching/{requirements,candidates,matches}`, `PATCH .../matches/{id}` (shortlist/decline), `POST .../matching/setup` (`http/mod.rs:141-146`). UI: Matching page (hidden unless the HR pack is on), shortlist button, reasons.
- Tests: `matching.rs::referral_and_requirement_emails_become_explained_matches` (Capgemini requirement, friend's referral, score 90, weak candidate filtered, decision survives re-run, tenant isolation), `matching_semantic.rs` x4 (synonyms, vector recall without skill overlap, embedding outage degrades, `daily_digest_sends_new_strong_matches_once_per_day`), web `MatchingPage.test.tsx` x3. All pass. Agent flow: `agents.rs::requirement_case_runs_to_an_interview`, `declined_submission_is_never_retried`.
- User flow today: enable HR pack, Matching, Setup; connect Gmail; referral and requirement mails arrive and are labelled; open Matching, pick a requirement, read the explained list, shortlist; morning digest email lists new strong matches (score 70+) at the alert digest hour; or start a Requirement agent case that drafts the submission for approval.
- Gaps:
  - Matching depends on the classifier assigning exactly `client_requirement` or `candidate_referral`; no accuracy measure for real mail (no golden set in tree), tests use the offline stub.
  - Digest is tied to `alert_settings.daily_digest` and the alert recipients; there is no Matching-specific digest setting or preview in the UI.
  - Only 5 attachments read per mail; client names (TCS etc.) come from the sender domain/body, no client master list.
  - LLM rerank hook is still "next" (roadmap). Dedup of the same candidate across senders is not asserted in tests.
  - "Referral review path not asserted" (wave3 note) is still true: no test covers the referral review item.

## 3. Earlier labelled items

### docs/wave3-plan.md R1-R9 table (labelled at commit b82254e, now stale)

| Wave3 label | Wave3 said | Verified now |
|---|---|---|
| R1 leave + referral | Working | Working for labelling/reviews (`pipeline.rs::timesheet_vertical_slice_end_to_end`, `manual_override_triggers_workflow_once`); referral review path still not asserted. |
| R2 alerts | Missing | Now Partial: engine, UI, escalation built and tested (see R3 above), but flat chain, only two alert kinds. |
| R3 performance | Missing | Still Missing. |
| R4 billable/non-billable | Missing | Still Missing (people directory not built). |
| R5 portal reconciliation | Missing | Still Missing (no CSV import, no tables). |
| R6 read all mail + attachments | Partial | Improved to mostly Working for attachment types (XLSX, PPTX, Word, PDF, images, audio, tested); remains Partial because of the 14-day backfill and 2 h AI window. |
| R7 team members' mail | Partial | Still Partial (admin-only connect, no per-person mapping, no mailbox filter). |
| R8 ask questions | Missing | Now Partial: Ask Meera + knowledge store exist; no metric/count tools. |
| R9 referral and requirement | Working (v1) | Working, plus semantic matching and daily digest now tested; LLM rerank still absent. |

Wave 3 task status (code-checked, not all re-run): 1 send/injection safety built (`send_safety.rs`, `security.rs`, tests green); 2 AI ledger and budgets built (`ai_ledger.rs`, `tests/ai_usage.rs`); 3 golden set + CI baseline not in this tree (`tests/golden/` absent; `label_health` page and `ai_category` tracking exist); 4 XLSX via calamine and all-label attachments done, job-lease heartbeat not found; 5 people directory not built; 6 `GET /threads/{id}` not routed (grep only finds a test string); 7 vector row delete on chunk removal exists (`knowledge.rs:276`); 8 rate limits/semaphores partly (`jobs/worker.rs`, `sources/nango.rs`).

### docs/roadmap.md (R1/R2/R3/R4/R5/R6/R7/R8/R9 use a different numbering from Shrikant's list)

| Roadmap row | Roadmap status | Actual |
|---|---|---|
| R1 leave + referral | Live | Live (labels, workflows, reviews). |
| R6 read all email + attachments | Live (bodies, CV/JD text) | Understated: all attachment types now read. But see R4 gaps above. |
| R9 referral to requirement | Semantic matching + digest built; LLM rerank next | Confirmed by tests. Rerank not built. |
| R2 alerts | Planned (M5) | Partly built ahead of plan (alerts engine + inbox + escalation); people directory and timesheet-missing not built. Roadmap is stale. |
| R4 billable/non-billable | Planned (M5) | Not built. |
| R5 portal match | Planned (M7) | Not built. |
| R3 performance | Planned (M6) | Not built. |
| R8 Ask agent | Planned (M8) | General Ask Meera built (not in roadmap); the metric/SQL half is not. |
| R7 team mail | Connect works, mapping in M5 | Confirmed: connect works, mapping missing. |
| M0, M1, M2, M3 | Mixed | M1, M2 (knowledge store), M3 (manual draft/send), M4 are live; golden set (M2) is not in tree. |

## 4. Gap list, ordered by client value

`docs/product-v1-plan.md` WP-A..F cover agents, learning, evals, designer, hardening and UI polish. None of them includes people, performance, billable policy or portal data; those need a new package ("WP-G Staffing operations": people directory, submissions/metrics, reconciliation) that depends on WP-B (agent registry) for the `hrms.*` agents.

| # | Gap | Req | Why it matters | Suggested owner |
|---|---|---|---|---|
| 1 | People directory: employee, emails, billable/non-billable, client, manager, joining date, role, daily/weekly targets (CSV import + UI) | R1 R2 R3 R5 R6 | Prerequisite for 2-8. Today nothing knows who is billable, who reports to whom, or when someone joined. | **new** (WP-G) |
| 2 | Submissions model: detect a recruiter's sent mail with CV attached, link to requirement and candidate, track outcome (interview/rejected/joined) | R1 R6 | Source of every performance number; only agent-driven submissions are recorded now. | **new** (WP-G), uses WP-B cases |
| 3 | Metrics API + corner widget (today, week, target vs achieved, interview rate), team view for managers | R1 R6 | The client's first headline ask; zero code today. | **new** (WP-G) backend; WP-F for widget |
| 4 | Ask Meera metric tools: exact counts per person/date range/since joining, quality (interview rate), cited | R6 | Makes "how many submissions since joining" answerable without guessing. | **new** (WP-G) + WP-B tool allowlist |
| 5 | Billable policy: client-domain/mailbox rule so only important mail and monthly timesheets reach the HR mailbox; month-end "timesheet missing" alert | R2 | Core business model of the client; privacy of client mail. | **new** (WP-G) + WP-B `hrms.timesheets` agent |
| 6 | Portal reconciliation: CSV import of portal hours/submissions, exact joins against timesheet mails and submissions, mismatch alerts | R3 | Whole second half of R3 is absent. | **new** (WP-G) |
| 7 | Layered escalation: first alert to the mailbox owner, then manager, then HR from the directory; new alert kinds (timesheet missing, below target, mismatch, correction needed) | R3 | Current chain is one flat workspace list, so alerts cannot reach "respective levels". | **new** (WP-G) on top of existing `alerts.rs`; UI in WP-F |
| 8 | Per-person mailboxes: non-admin members connect their own Gmail, inbox filter by mailbox/person, scoping rules, two-mailbox isolation test | R5 | Needed for "agent reads all team members' mail" at scale and for consent. | **new** (WP-G); isolation tests in WP-E |
| 9 | Read-everything mode: configurable full-history backfill (not 14 days / 100 mails), a budgeted "read all" background pass that runs insight + attachments + embeddings on old mail, and a "N of M read" progress indicator | R4 | Today most history is stored but not understood. | WP-E (hardening/jobs) with WP-F progress UI |
| 10 | Real-model quality proof: golden set for labels and matching, run in CI | R7 R4 | All tests use offline stubs; matching depends on correct labelling of `client_requirement` / `candidate_referral`. | WP-D |
| 11 | Matching extras: LLM rerank, matching-specific digest settings and UI preview, attachment cap above 5, referral review-path test | R7 | Polish for the one capability already working. | WP-A (context) / WP-F (UI) |
| 12 | Doc drift: roadmap and wave3 status tables are stale (alerts, Ask Meera, attachments shown wrong) | all | Misleads planning. | WP-E (docs) |

## 5. Bottom line

Of Shrikant's seven asks, one is Working (R7 matching), four are Partial (R3 alerts only, R4, R5, R6 Q&A only) and two are
entirely Missing (R1 performance widget, R2 billable/non-billable). The missing items share one root: there is no people
and submissions data model. Build gap 1 and gap 2 first; most of the rest follow.
