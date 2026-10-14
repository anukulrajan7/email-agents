# Roadmap: every requirement, one plan

Status: 15 Oct 2026 (see `status-2026-10-15.md` and `features.md`) · combines `plan-staffing-agent.md` (what to build) and `plan-ai-automation.md` (how the AI works)

Decisions locked: each person connects their own Gmail · portal data by CSV export · quality = interview rate ·
first demo = referral ↔ requirement matching · embeddings via API (`baai/bge-m3` through OpenRouter, no-retention
routing) · ~100-email golden set labelled by the team · humans send; drafts auto-save to Gmail.

## 1. Requirement → how it's solved → when

| # | Shrikant's requirement | How (RAG / rules / tools) | Phase | Status |
|---|---|---|---|---|
| R1 | Leave + referral handling | Workflows on labels | – | ✅ Live |
| R6 | Read all emails **and attachments** | Gmail backfill + PDF/DOCX text | – | ✅ Live (bodies, CV/JD text) |
| R9 | Referral profiles ↔ client requirements | Extraction + skill taxonomy + explainable score + **vector recall** + daily digest → LLM rerank | M1 → M4 | ✅ Semantic matching + digest built (M4) · LLM rerank next |
| – | Labels that learn the team's meaning | **RAG few-shot** from confirmed labels; Qwen → Kimi escalation | M0 → M2 | ✅ Live (corrections feed few-shot; knowledge store) |
| – | Replies in the team's style, saved to Gmail | **RAG** over sent/approved replies; auto-save drafts | M3 | ✅ manual save/send live |
| R2 | **Alerts:** important email missed; trigger events | **Rules engine** (not RAG) + LLM "needs reply?" judgement | M5 | ✅ Live (owner → manager → HR escalation) |
| R4 | Billable vs non-billable staff | People directory (type, client, manager, targets) | M5 | ✅ Live (suppressed mail policy) |
| R5 | Match email vs portal data; alerts per level | CSV import → exact joins → **mismatch alerts** up the chain | M7 | ✅ Live |
| R3 | **Performance:** day/week, target vs achieved, corner widget | Submissions extracted from sent mail (**RAG links** submission → requirement/candidate) → metric views | M6 | ✅ Live |
| R8 | Ask questions about all data; quality since joining | **Ask agent:** RAG (what/why, with sources) + SQL tools (exact counts) | M8 | Partial: Ask Meera is live with sources; exact HR counts not wired to it yet |
| R7 | Read team members' mail | Each person connects Gmail; mailbox → person mapping | M5 | Mapping and scoped inbox live · members connecting their own Gmail is after v1 |

## 2. Alerts: what fires, who gets it, how it escalates

One **rules engine**: trigger (an event or a schedule) → condition → action, with an escalation chain. Alerts are
**deterministic rules**, so they're predictable and auditable. The LLM only answers narrow questions such as
"does this email need a reply?", which is already stored on every email.

| Alert | Trigger | First to | Escalates to (if unresolved) |
|---|---|---|---|
| Important email not answered | label/priority = important, no reply in the thread after N hours (default 4 working hours) | mailbox owner | manager → HR (after 1 more day) |
| Approval pending | `leave` / `approval` email older than N hours with no decision | approver | approver's manager |
| Timesheet missing (billable staff) | month end + 3 days, no `timesheet` email from that employee | employee | account manager → HR |
| Portal mismatch | CSV hours ≠ hours in timesheet email, or a submission missing in the portal | employee | manager → HR |
| Below target | recruiter's submissions today/this week < target at 4 pm / Friday noon | recruiter | team lead (weekly summary) |
| Strong new match | match score ≥ 80 for an open requirement | owning recruiter | – |

Each alert has: in-app **Alerts** inbox (resolve / snooze / reassign), an **email** notification (Hostinger SMTP, already
working), and a **morning digest**. Every alert links to its evidence (the email, record or CSV row) and is audited.
Escalation levels come from the people directory (`manager_id` chain).

## 3. Performance: where the numbers come from

1. **People directory:** name, emails, billable/non-billable, client, manager, joining date, role (recruiter…),
   daily and weekly submission targets. Import by CSV or edit in the UI.
2. **Submissions:** a recruiter's *sent* email to a client with a CV attached = a submission. Extraction gives the
   candidate and client; **RAG links it** to the right open requirement and candidate record (semantic match on
   JD/CV text), and a human confirms ambiguous links.
3. **Outcomes:** client replies labelled `submission_update` → interview / rejected / selected / joined.
4. **Metrics (exact SQL views, not RAG):** submissions per day/week vs target, **interview rate** (the agreed
   quality metric), full funnel since joining date, per recruiter and per team.
5. **UI:** a corner widget on every page ("Today 3/5 · Week 14/25 · Interview rate 32%"), a team dashboard for
   managers, and a per-person history.

## 4. Build order (each milestone is shippable and demoable)

| Milestone | Scope | Demo |
|---|---|---|
| **M0** (ready) | Confident labels auto-approved; Qwen → Kimi escalation; sanitized HTML email display (links, images) | Review queue holds only unsure emails; emails look like Gmail |
| **M1** ✅ | Matching v1 (extraction + explainable score + shortlist) | Done |
| **M2** ✅ | Knowledge store (chunks + bge-m3 embeddings in SQLite + hybrid search) · RAG labelling · golden set (~100) + accuracy page | "Labels improved from X% → Y% on our own emails" |
| **M3** ✅ | RAG drafting + auto-save workflow drafts to Gmail | Drafts waiting in Gmail, in the team's voice |
| **M4** ✅ | Semantic matching (skill taxonomy, vector recall, semantic-fit score) + daily match digest · LLM rerank hook left open | Synonym matches found; morning email |
| **M5** ✅ | People directory · rules engine · **alerts** (missed email, approval pending, timesheet missing) + escalation + Alerts inbox | Unanswered Capgemini email escalates to the manager |
| **M6** ✅ | Submissions + outcomes → **performance** dashboard + corner widget | Recruiter sees today vs target; manager sees the team |
| **M7** ✅ | Portal CSV import → reconciliation → **mismatch alerts** by level | Hours mismatch flagged to employee, then manager |
| **M8** (partial) | **Ask agent** (RAG + metric tools, cited answers) | "How many submissions did Priya do since joining, and what % got interviews?" |

Effort guide: M0–M4 are mostly in the existing stack. M5–M6 add the people/rules/metrics model. M7–M8 build on all of it.
