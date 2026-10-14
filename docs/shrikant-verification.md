# Shrikant requirements: verification audit (14 Oct 2026)

Original audit checked at `dbf45f8` on branch `wp-verify` (main plus Packs M1). Method: read code and migrations, then ran the tests that
claim to cover each item. Strict rule: code with no test and no UI is **Partial**; nothing at all is **Missing**.

**Update after WP-G (branch `wp-g`, migration 0035).** The people / performance / billable / portal work is now built and each
requirement was run end to end through the HTTP API and the real job pipeline in `apps/api/tests/staffing_e2e.rs`
(`r1_…` to `r7_…`, one test per requirement: seeded people from a CSV, mail with attachments, portal CSVs, the matching
pipeline). Section 1 shows the new verdicts; the sections after it keep the original evidence and now mark what changed.
Tests behind the new verdicts: `staffing_people`, `staffing_submissions`, `staffing_metrics`, `staffing_policy`,
`staffing_portal`, `staffing_escalation`, `staffing_scope`, `staffing_roles`, `staffing_packs`, `staffing_e2e` (all green), web
`team.test.tsx`, `hrms.test.tsx`, `AppShell.test.tsx`. Everything still uses the offline stubs for AI, and Gmail is simulated
by storing mail through `ingest::persist_message` (the path every mailbox adapter uses); nothing here proves real-model quality
or live Gmail behaviour.

Original run (all green): API `agent_profiles, agents, alerts, assistant, attachments, gmail_backfill, inbox_insights,
knowledge, matching, matching_semantic, packs, pipeline` (84 tests), web `alerts, matching, assistant, inbox, agents, packs`
(96 tests). Golden/live-LLM evals were not run.

## 1. Verdict table

| Req | Client ask | Before | Now | Why (end-to-end test in `staffing_e2e.rs`) |
|---|---|---|---|---|
| R1 | Individual performance widget: today, this week, target vs achieved | Missing | **Working** | `r1_…`: people CSV with targets, three CVs sent to a client are detected from sent mail, the recruiter's `metrics/me` shows 3 of 5 today, interview rate moves when the client replies, the team lead sees the team, a person outside the directory sees no widget, below-target alert at 4 pm. Needs each recruiter's mailbox connected (admin action). Outcomes are read by plain phrases, not by a model. |
| R2 | Billable vs non-billable; billable = important mail + monthly timesheets only | Missing | **Working** (as specified) | `r2_…`: billable Ravi's client mail is never stored from a personal mailbox, his timesheet is read through the HR mailbox, non-billable keeps the full mailbox, and a missing September timesheet alerts him three days after month end. "Important mail" is not filtered further: the rule is "mail a billable person sends, outside the HR mailbox, is stored hidden (`suppressed`) and released if the policy changes". Cc and recipients never trigger it. |
| R3 | Match email data with portal data; reminders/approvals escalate by level | Partial / Missing | **Working** | `r3_…`: portal hours CSV (with a validation report) is joined exactly against the timesheet mail, the 160 vs 152 h difference raises `portal_mismatch`, and it goes employee, then manager, then HR on configurable delays, and closes when the portal is corrected. Hours are read from the mail by pattern ("Total hours: N"); a mail without a readable number is counted as unverified, not as a mismatch. No "correction needed" alert kind. |
| R4 | Read all emails and attachments first, produce outputs later | Partial | **Partial** (unchanged) | `r4_…` pins the gap: every mail is labelled on arrival, but summaries of mail older than `AI_AUTO_HOURS` wait until someone opens it; backfill is still 14 days / 100 mails by default. Not in WP-G scope. |
| R5 | Agent reads all team members' company email | Partial | **Partial** (improved) | `r5_…` and `staffing_scope.rs`: mailbox-to-person link, inbox filter by mailbox or person, two-mailbox isolation test, managers may scope to their reports and not upward. Still open: only admins can connect a mailbox (no per-person consent flow), and scoping is a view filter, not an access wall (every member can still open the shared inbox). |
| R6 | Agent answers questions on all data; monitors submissions + quality since joining | Partial / Missing | **Partial** | Monitoring is **Working**: submissions, outcomes, joining date and interview rate are exact SQL, in the API and the Performance page (`r6_…` computes "4 submissions since joining, 50% interviewed" for Priya). Q&A is **not wired**: `staffing::metrics::{find_people, person_metrics, team_metrics}` are the read-only hook documented in `docs/api.md`, but `src/assistant/*` belongs to another package and was not touched, so Ask Meera cannot yet say these numbers. No scheduled performance summary. |
| R7 | Daily referral profiles matched against client requirements | Working / Partial | **Working** (unchanged) | `r7_…`: the existing pipeline produces the explained 90-point match, and now a CV the recruiter sends is linked to that same candidate and requirement. Matching gaps below still stand. |

## 2. Evidence per requirement

### R1 Performance widget: Missing (original audit; now Working, see section 1)
- Searched `apps/api/src`, `migrations`, `apps/web/src` for people/performance/target/billable/joining: no hits beyond unrelated words. No migration has a `people`, `targets` or `submissions` table. `nav.ts` has no dashboard; no corner widget in `AppShell.tsx`.
- Closest thing: `submit_candidate` agent tool (`agents/tools.rs:824`) and `cases` rows, tested in `agents.rs::requirement_case_runs_to_an_interview`. That is an approval-gated agent action, not a count of a recruiter's submissions, and it ignores submissions that recruiters send by hand from Gmail.
- A user can do today: nothing for this. They can only open Agents and read cases one by one.
- Gap: people directory (joining date, role, daily/weekly target), submission detection from sent mail with CV attached, outcome tracking, metric endpoint, corner widget.

### R2 Billable / non-billable: Missing (original audit; now Working, see section 1)
- No `billable` or employee-type concept in code. Mailbox ownership exists only as `email_sources.connected_by_user_id` (`migrations/0002`), which links a mailbox to a login user, not to an employee with a type or client.
- Nothing recognises "client email ID" or filters which mail reaches the HR/timesheet mailbox. Billable staff would simply connect Gmail like anyone and all their client mail would be read.
- Partial building blocks: `timesheet` label and `timesheet_review` workflow (`domain.rs:63,171`), asserted by `pipeline.rs::timesheet_vertical_slice_end_to_end`; preset `acknowledge_timesheets` (`automations.rs:335`). These treat timesheet as one label for everyone.
- User can do today: label/auto-acknowledge timesheet mail and open a review item. They cannot say who is billable, nor expect monthly timesheet or missing-timesheet tracking.
- Gap: employee type + client mapping, mailbox policy ("billable: only important + timesheet leave the person's mailbox"), month-end timesheet-missing alert.

### R3 Reconcile with portal; escalate by layer: Partial (original audit; now Working, see section 1)
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

### R5 Agent reads all team members' mail: Partial (original audit; improved, still Partial)
- Connecting: `POST /workspaces/{ws}/sources/nango-session` and `nango-connections` (Admin only, `http/sources.rs:171,200`), so an admin must connect each mailbox, or each person must be made admin. Not a per-person consent flow.
- Many Gmail sources per workspace are supported (`UNIQUE(kind, external_connection_id)`); `connected_by_user_id` identifies the owner, used in `alerts.rs:276`, `send_safety.rs:339`, `memory_learning.rs:340`, and per-case `agent_user_id` (`migrations/0026`, `agents/triggers.rs:91`) so the agent adapts to the mailbox owner's profile. Profiles tested in `agent_profiles.rs` (8 tests) and `agentProfiles.test.tsx`.
- Gaps: no test connects two mailboxes and checks isolation; inbox has no per-mailbox/per-person filter in the list API (`http/messages.rs`); no people directory; admin-only connect; everyone with workspace access sees all mail (no scoping). Wave3 item 5 (people + per-person mailboxes) is not built.

### R6 Agent answers questions and monitors performance: Partial / Missing (original audit; monitoring now Working, Q&A still not wired)
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

Status column added after WP-G. Numbers 1-8 were the "WP-G Staffing operations" package.

| # | Gap | Req | Status after WP-G |
|---|---|---|---|
| 1 | People directory: employee, emails, billable/non-billable, client, manager, joining date, role, daily/weekly targets (CSV import + UI) | R1 R2 R3 R5 R6 | **Done.** Generic for every workspace (billable and client only with the staffing pack). Super admin writes, others see themselves and their reports. CSV import with dry run and a row-by-row report. `staffing_people.rs`, `staffing_roles.rs`. |
| 2 | Submissions model: detect a recruiter's sent mail with CV attached, link to requirement and candidate, track outcome | R1 R6 | **Done.** Scan every 15 min plus on demand; ambiguous links wait for a person (`needs_confirm`), false positives can be dismissed, outcomes read from the thread or `submission_update` replies. `staffing_submissions.rs`. Not done: the `submit_candidate` agent action does not write to this table yet (it is another package's code), so agent-made submissions are only counted once they appear in the sent mailbox. |
| 3 | Metrics API + corner widget (today, week, target vs achieved, interview rate), team view for managers | R1 R6 | **Done.** Pluggable exact-SQL metric definitions; generic metrics for other packs. `staffing_metrics.rs`, web `team.test.tsx`. No per-person history page. |
| 4 | Ask Meera metric tools | R6 | **Hook only.** `staffing/metrics.rs` exposes the read-only functions and `docs/api.md` documents them; wiring the assistant tool allowlist is the assistant package's job. |
| 5 | Billable policy and month-end timesheet-missing alert | R2 | **Done.** `staffing_policy.rs`. Needs `hr_source_id` set (Timesheets page); until then the policy is off. |
| 6 | Portal reconciliation | R3 | **Done.** `staffing_portal.rs`. CSV column names are assumed (`email,month,hours` and `email,date,candidate,client`); the real portal export format has not been seen. |
| 7 | Layered escalation and new alert kinds | R3 | **Done** for `timesheet_missing`, `below_target`, `portal_mismatch` and owner-first routing of the existing alerts. `staffing_escalation.rs`. Not done: "correction needed"; reassigning an alert to another person. |
| 8 | Per-person mailboxes: inbox filter by mailbox/person, two-mailbox isolation test | R5 | **Partly done.** Filter and isolation test done. Not done: non-admin members connecting their own Gmail (consent flow), and scoping as an access wall. |
| 9 | Read-everything mode: full-history backfill, budgeted "read all" pass, "N of M read" indicator | R4 | Open (WP-E / WP-F). |
| 10 | Real-model quality proof: golden set for labels and matching in CI | R7 R4 | Open (WP-D). Note: submission outcomes use phrase rules; a golden set for `submission_update` replies is needed before trusting them on real mail. |
| 11 | Matching extras: LLM rerank, matching digest settings, attachment cap above 5, referral review-path test | R7 | Open. |
| 12 | Doc drift: roadmap and wave3 status tables are stale | all | Open. |

## 5. Bottom line

After WP-G, of Shrikant's seven asks four are **Working** (R1 performance widget, R2 billable policy, R3 portal reconciliation
and layered escalation, R7 matching) and three are **Partial** (R4 read-everything depth, R5 per-person mailboxes without a
consent flow, R6 monitoring done but Ask Meera not yet using the metric functions). The biggest remaining risks are not in
code structure but in unproven inputs: real portal CSV layouts, real timesheet wording, and the phrase rules that read client
replies, none of which has been tried on real mail.
