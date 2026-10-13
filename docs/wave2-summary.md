# Wave 2 summary: what we built (14 Oct 2026)

Live at **https://mail.anukulraj.tech** · `main` = `2f22ef9` · 3 deploys, all green · backend 18 test suites,
web 110 tests, clippy 0 warnings · migrations 0012–0017 applied in production.

Product rules adopted: **AI is always on**, model names are never shown, **every email looks like Gmail**, labels and
memory are **automatic**, humans only correct.

## 1. What shipped

| Area | What the user sees | How it works | Commit |
|---|---|---|---|
| **Understanding** | "What they're asking" card: summary, asks, deadline, needs reply, missing info | `ANALYZE_MESSAGE` job → `message_insights`; drafting model | `06b25f1` |
| **Safer drafts** | `[NEEDS: …]` gaps highlighted, checklist; Send blocked until filled | Drafter answers each ask or writes `[NEEDS]`; server refuses send (409 `draft_incomplete`) | `06b25f1`, B, D |
| **Gmail-like reader** | One reader for HTML and text: avatar, name, date, auto-height body, collapsed quotes, link chips, attachments | `EmailReader`; sandboxed iframe (no scripts) | D `276e537` |
| **Clean text** | No `<!--[if mso]>`, `<https://…>` wrappers or duplicated blocks | `text_clean.rs` at ingest + re-clean job; old Gmail rows re-fetched for HTML | A `b229412` |
| **Mail direction** | Our own sent mail is not triaged or drafted against | `email_messages.direction`, Gmail SENT label / mailbox address; `threads::thread_has_reply_after` | A |
| **Inbox** | Important first (needs reply → priority → newest); one-line summary; no "Approve label" | `GET /messages?sort=importance`; status `processed` ends the stuck tracker | A, D |
| **Labels** | AI-suggested labels; new label re-checks recent "Other" mail; **Health** tab (auto rate, corrections, Other volume, advice) | `POST /labels/suggest`; re-classify ≤100 recent Other; `GET /labels/health`; corrections write `ai_category` + `confirmed` | A, E, `285635c`, `19a5a32` |
| **Automations** | "When X → do Y" builder in sentences, presets, run log; defaults *Reply to questions* (draft + save to Gmail Drafts) and *Chase important* (reminder after 4 h) | `automations`, `automation_runs`, `RUN_AUTOMATIONS`; auto-send needs allowed domains, confidence ≥ 0.85, daily cap, no `[NEEDS]`, no bot senders | B `28fbc65`, E `95528a2` |
| **Self-learning memory** | "What the assistant has learned": contacts, clients, standing answers, preferences; pin / confirm / delete | `LEARN_FROM_MESSAGE` / `LEARN_FROM_REPLY`; external facts capped at 0.7 until a team reply confirms; drafts use ≥ 0.6 | F `8140791`, G `4f5ebce` |
| **Alerts** | Alerts inbox + badge; email alerts via Hostinger SMTP; escalation; daily digest; "Send test alert" | `alerts.rs` every 15 min: important unanswered, approval pending, automation failed / reminder; escalation after 24 h | F, G, `8140791` |
| **RAG** | Labels learn this team's meaning; drafts follow the team's past replies | Knowledge store: chunks + FTS5 + sqlite-vec, `baai/bge-m3`, hybrid search (RRF); `GET /search` | C `b348449` |
| **Matching** | Synonyms understood; similar profiles found; "Profile closely matches the role" reason; morning digest of strong matches | 83-skill taxonomy, profile embeddings, recall top-20; weights skills 50 / exp 20 / location 15 / semantic 15 | `2f22ef9` |

## 2. Shrikant's requirements now

| R | Status | Notes |
|---|---|---|
| R1 leave + referral | ✅ Working | Workflows + automations |
| R2 alerts | ✅ v1 live | Email alerts + escalation to configured recipients; manager chain needs the people directory |
| R6 read mail + attachments | 🟡 Partial | Full bodies + HTML; attachment text only for referral/requirement, no XLSX |
| R7 team members' mail | 🟡 Partial | Admin connects mailboxes; no per-person mapping yet |
| R8 ask questions | 🟡 Foundation | Search API live; Ask agent not built |
| R9 referral ↔ requirement | ✅ v2 live | Semantic matching + digest; LLM rerank hook empty |
| R3 performance, R4 billable/non-billable, R5 portal match | ❌ Not started | Wave 3 |

## 3. How we worked

A shared contract (`docs/wave2-tasks.md`) was written before code, so 9 agents (7 builders, 2 follow-ups) built in parallel
in separate git worktrees, plus 1 read-only CTO/AI review. The integrator merged branch by branch, ran all tests after
each merge, wired cross-agent hooks (alerts ↔ automations, reply detection ↔ direction, re-index ↔ text cleaning),
tested migrations on a copy of the old database, and deployed. The review's findings (sent mail being triaged, unsafe
auto-send preset, prompt-injection gaps) were pushed to running agents before they finished.

## 4. Open items (see `docs/wave3-plan.md`)

1. Send and prompt-injection hardening; AI call ledger and budgets; golden set and accuracy checks.
2. People directory + per-person mailboxes (R4/R7) → alerts to the mailbox owner and manager chain.
3. Submissions + performance widget (R3); portal CSV reconciliation (R5); Ask agent (R8).
4. Conversation (thread) view; search screen; LLM rerank for matching; calibrate similarity cut-offs on real data.
5. Housekeeping: delete the obsolete remote branch `worktree-agent-a4ec701c3db8f1a7b` (its work is already on `main`);
   agents D and C bypassed the worktree git guard via `/usr/bin/git` — tighten the guard if that matters.

## 5. Try it

1. Hard refresh; open a few emails (old Gmail mail regains HTML within ~10 minutes of a sync).
2. **Alerts → Settings**: add your email, **Send test alert**.
3. **Automations**: check the two defaults are on; watch Gmail **Drafts**.
4. **Labels → Suggest labels**, then **Health**.
5. **Matching**: look for "Has related skill" and "Profile closely matches" reasons.
