# Wave 3 plan (from the CTO / AI-engineering review, 14 Oct 2026)

## Shrikant's requirements: honest status at commit b82254e (before wave 2 merges)

| R | Verdict | Note |
|---|---|---|
| R1 leave + referral | Working | Referral review path not asserted in tests |
| R2 alerts | Missing → wave 2 (agent F) | Needs reply detection (mail direction, agent A) |
| R3 performance | Missing | Needs sent-mail direction + submissions (M6) |
| R4 billable/non-billable | Missing | People directory (M5) |
| R5 portal reconciliation | Missing | CSV import (M7) |
| R6 read all mail + attachments | Partial | Attachments read only for 2 labels, PDF/DOCX/TXT, ≤5 files; no XLSX |
| R7 team members' mail | Partial | Admin-only connect; no per-person mapping or mailbox filter |
| R8 ask questions | Missing | Knowledge store lands in wave 2 (agent C); Ask agent in M8 |
| R9 referral ↔ requirement | Working (v1) | LLM/attachment extraction untested; no daily digest |

## Fixed during wave 2 (pushed to running agents)
- Mail direction: SENT mail is stored as `outbound`, never classified/drafted; `thread_has_reply_after` helper (A).
- No preset auto-sends; `send_reply` needs `allowed_domains`; server refuses any send with `[NEEDS:` (B).
- Memory extraction: fenced untrusted text, word-boundary redaction, external facts capped until confirmed (F).

## Wave 3 tasks (in order)
1. **Send + injection safety**: escape prompt fences everywhere (`</email>` breakout), word-boundary redaction in
   `ai/mod.rs`, n-gram check that drafts don't leak `<team_notes>`, calibrated confidence for auto-send.
   *Accept:* injection fixtures produce no leak and no send.
2. **AI call ledger + budgets**: `ai_calls` (task, model, prompt version, tokens, latency, finish_reason, cost,
   outcome); per-workspace daily budget with fallback to the fast model. *Accept:* mocked `usage` recorded.
3. **Golden set + eval**: write `ai_category`/`confirmed` on every label change (corrections are the human signal now
   that "approve" is gone); fixtures in `tests/golden/`; per-label precision page. *Accept:* CI fails below baseline.
4. **Context budget + robustness**: strip quoted history; per-attachment budget (today 20k chars are cut to 6k);
   XLSX via `calamine`; attachments for all labels; `spawn_blocking` + `catch_unwind` for PDFs; job lease heartbeat
   (lease 120 s is shorter than classify+escalate); refresh insight/extraction when content changes (today
   `ON CONFLICT DO NOTHING` keeps stale snippet-based insights); one analysis per message (no inline + job race);
   `finish_reason=length` → retry with more tokens, schema rejection → fallback model.
5. **People directory + per-person mailboxes (R4/R7)**: `people`, CSV import, `email_sources.person_id`, operators
   connect their own Gmail, inbox scoped per mailbox.
6. **Conversation view + failure visibility**: `GET /threads/{id}`, drafter sees the thread, dead jobs raise
   `system` alerts, retention purge for `other` mail (DPDP).
7. **Knowledge hygiene**: delete `knowledge_vec` rows with their chunks, FTS update trigger, transaction around
   `extract_record` writes.
8. **Rate limits**: stagger mailbox polls, per-workspace Gmail/AI concurrency caps.

Doc drift to fix while merging: `roadmap.md` R6 "Live", stale `ProcessingStatus` in `api.md`, embeddings described
three ways (decided: `baai/bge-m3` via OpenRouter, 1024-d).
