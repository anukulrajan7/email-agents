# Universal platform: design decisions and milestones (14 Oct 2026)

Inputs: architecture audit (main @ 9775695), specialist-agents design, research brief (`docs/research-2026-10.md`).

## Decisions
- **Automations become the versioned workflow engine** (UI name "Workflows"); legacy HR `workflows` → HR-pack presets, runs kept read-only.
- **Packs** (`src/packs.rs`, `workspace_packs`): labels, presets, reply style, features (matching, requirement agent), specialists per use case. HR/recruiting is one pack; new workspaces default to `general`.
- **One lead agent per workspace** (e.g. Meera) routes to **specialist desks** (data, not processes): label = desk, deterministic rules first, no extra LLM call; shadow routing before cutover.
- Staffing desks: Arjun (client requirements), Kavya (candidates), Rohan (timesheets), Ananya (leave/HR), Meera (general). Support / Sales / Finance / Personal packs designed.
- **Per-member profiles** (role, tone, sign-off, VIPs, approval per action); outgoing mail always in the mailbox owner's voice, never mentions AI.
- **Prompt library** (`src/ai/prompts.rs`, versioned): generic prompts; `[staff]` vs `[learned]` notes (learned facts never instructions); `<asks>` treated as untrusted; no "HR Operations" defaults; planner never sees raw email (quarantined extractor).
- **Models (no Anthropic):** standard draft `deepseek/deepseek-v4-flash`, premium draft + planner `deepseek/deepseek-v4-pro`, classify/judge `qwen/qwen3.8-flash`, Kimi K2.6 fallback; vision `qwen/qwen3-vl-30b-a3b-instruct`; audio `qwen/qwen3-asr-1.7b`.
- **Router:** deterministic conditions → shared AI cache → priority; `message_claims(message_id, 'reply')` so only one path drafts/replies per email.
- **Approvals:** `approval_requests` with content hash (edit voids approval); auto-send = min(workspace cap, step, owner profile, source) + all safety gates.
- **SQLite:** never rebuild referenced tables (cases, drafts, automations, email_messages); add columns, enforce values in Rust.

## Milestones (in order)
1. M1 packs + de-HR defaults (support workspace gets no HR labels/workflows/extraction/requirement cases).
2. M2 versioned definitions (`automation_versions`, backfill v1).
3. M3 step registry + executor + dry-run (`src/workflow/*`): classify (custom categories), summarize, extract (schema), draft, label, record, approval, notify, provider draft, send, follow-up, webhook, start agent.
4. M4 router + exclusivity; HR templates migrated.
5. M5 approvals + send permission (content hash, pass^k idempotent sends).
6. M6 natural-language → workflow (validate + repair ≤2) + test on a sample; 20–50 golden goals.
7. M7 goal-first onboarding (end-to-end MVP test).
8. M8 agent-proposed automations, manual/scheduled triggers, signed webhooks.
Parallel: prompt library M0 (snapshot-tested), specialist tables + shadow routing, attachments (Excel/PPT/image/audio), voice agent.

## Attachments as text (`src/attachments/`, job `extract_attachments`)
- Readers: spreadsheets (calamine + csv: per sheet a markdown table of the header and first 200 rows, ≤ 50 columns, cells ≤ 200 chars, plus row/column counts), PPTX / DOCX (hidden `w:vanish` runs and tracked deletions dropped) / TXT / MD / HTML / RTF, PDF via `matching::attachment_text`, images (`VisionReader`, ≤ 8 MB, ID numbers masked to the last 4), audio (`AudioReader`, ≤ 25 MB; WAV and MP3 are cut into ~45 s pieces because the upstream times out near 60 s, other containers are sent whole).
- Stored on `attachments`: `text_content` (also feeds search and matching), `extract_kind`, `extract_status` (done|failed|skipped), `extract_error` (plain words, never a provider message), `extract_meta_json` (UI facts).
- Job `attachments:<message_id>` runs on ingest; readers that need no model always run. Images and audio follow the AI rule (new mail, within `AI_DAILY_BUDGET_USD`); old mail is read when opened (`attachments:<id>:open` from `POST /messages/{id}/insight`). After new text: re-index, and the insight is refreshed once (`message_insights.attachments_hash`) with attachment text fenced as untrusted (≤ 3,000 chars each, ≤ 8,000 total).
- Config: `OPENROUTER_VISION_MODEL_ID` (default `qwen/qwen3-vl-30b-a3b-instruct`), `OPENROUTER_AUDIO_MODEL_ID` (default `qwen/qwen3-asr-1.7b`); the audio summary uses the classifier model. Empty id or no `OPENROUTER_API_KEY` = those files are skipped.
- Gap: attachments of forwarded (Resend) mail have no download handle in our model (`provider_ref` is empty), so they are recorded as skipped ("Files on forwarded mail can't be read yet"). Resend's receiving API can fetch them (`GET /emails/receiving/{id}/attachments/{attachment_id}`); wiring that is the follow-up.
