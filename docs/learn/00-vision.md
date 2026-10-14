# 00. Vision: what Post Room is and why

**Goals.** After this module you can say, in one minute and in plain words, what Post Room does, who it is for,
what makes it safe, and what is next.

**Prerequisites.** None. **Reading time.** 30 minutes.

## 1. The one-line idea

Post Room reads a team's email, understands it, and gets routine work done: labels mail, writes replies in the
team's voice, follows up, and keeps track of people, timesheets and requirements. A person stays in charge.

The product name inside the code is `email-ai-api` (`apps/api/Cargo.toml:2`). The user-facing name is Post Room.
The internal north star is **"Muse for email"**: one personal, proactive assistant per person, plus one expert per
field, all sharing one safe runtime.

## 2. Who it serves

| Person | What they get |
|---|---|
| A team member (recruiter, support rep, sales rep) | **Meera**, their own helper, works only on their mailbox, in their tone, with their memory. |
| An operator who works the shared inbox | Labels, summaries, drafts with gaps marked, one approval inbox ("Waiting for you"). |
| An admin | Packs to switch on, agents to tune, levels to grant, AI health and cost pages. |
| An HR or staffing company (the first customer shape) | The **HR and recruiting pack**: leave, timesheets, onboarding, requirements and candidates, payroll, portal reconciliation, performance. |

The first real requirements came from a staffing firm (see `docs/roadmap.md` section 1, "Shrikant's requirement").
That is why HRMS is one pack among several, not the whole product.

## 3. The ideas that shape everything

### 3.1 An AI helper earns freedom

`docs/architecture.md` section 1 says it best: *a new helper only watches. Then it drafts. Only after it has proven
itself on real mail, and an admin agrees, may it send routine replies on its own, and even then it can be stopped
for ten minutes before anything leaves. One bad moment and it loses a level automatically.*

The four levels (`apps/api/src/agents/autonomy.rs:45`, `effective_level`):

| Level | Name | What it may do |
|---|---|---|
| 0 | Observe | Shadow cases only. Drafts are scored against what a person sent. Nothing is stored for the user, nothing is sent. |
| 1 | Draft | Real drafts. Every send asks a person. |
| 2 | Routine send | Replies to the original sender, same thread, known contact, after a 10-minute hold with Stop. |
| 3 | Autonomous | Level 2, plus replies to any participant and first replies. |

Analogy: a new hire in a company. Week one they sit next to you. Then they write drafts you review. Then they
answer routine mail alone, but you can still catch it before it goes out. One serious mistake and they are back
to reviewing.

### 3.2 Guarded sends

Every send in the system goes through one function, `replies::send_draft`
(`apps/api/src/replies.rs:477`). There is no second door. That function runs the send-safety checks
(`apps/api/src/send_safety.rs:572`), the injection gate, the leak check, and only then schedules the send with a
30-second undo (`SEND_UNDO_SECONDS` in `apps/api/src/config.rs`).

Some things always ask a person, at every level. This is the **sensitive floor**: money, legal, security words,
attachments, new recipients, `[NEEDS: ...]` gaps and the agent's own `never_do` rules
(`apps/api/src/agents/autonomy.rs:150`, `blocker`).

### 3.3 Packs

A pack is a switch that adds labels, agent templates, workflows and (for HR) pages. General is always on. Support,
Sales, Finance, Personal and HR and recruiting are optional (`apps/api/src/packs.rs`,
`apps/api/src/agent_registry/templates.rs:472`, `TEMPLATES`). Each pack brings one expert: Ishaan (support), Kabir
(sales), Vikram (finance), Nisha (personal scheduling). HR keeps a team: Ananya, Rohan, Sneha, Arjun, Aditya.

### 3.4 Cheap by default, model names hidden

Model ids are configuration (`OPENROUTER_*` and `AGENT_*` variables). They are never shown to users; the AI usage
page shows task names like "Labelling" and "Drafts" (`apps/api/src/ai_ledger.rs:380`, `task_label`). A Playwright
test, `apps/web/e2e/no-model-names.spec.ts`, fails the build if a model name leaks into the UI.

### 3.5 A person's data stays theirs

Personal mail is invisible to admins. Agents started by admins refuse personal mail
(`apps/api/src/agents/triggers.rs:68`, `personal_refusal`). Every workspace route checks membership and every query
filters on `workspace_id`; a test walks every route to prove it (`apps/api/tests/tenant_isolation.rs`).

## 4. What "good" looks like

From `docs/research-2026-10.md` (market section) and `docs/agent-building-guide.md`:

- **Time saved** the user can feel: a draft is waiting, in their voice, with the gaps marked.
- **No surprises**: nothing leaves without a person unless they chose that, and even then there is a Stop button.
- **Predictable cost**: about $0.0005 per email labelled, about $0.001 with agents on (targets in
  `docs/ai-engineering-review.md` section 1). A daily budget stops automatic reading when spent.
- **Trust you can inspect**: every send, approval, level change and pause writes an audit row; every model call
  writes a ledger row; every near-miss writes an incident.

Not good, for an MVP (same research brief): fully autonomous sending from day one, thousands of integrations,
multi-agent swarms, voice as the main interface.

## 5. Where the product is today

`docs/status-2026-10-15.md` is the honest page. Short version: the product is a working, safe, cost-controlled
mail assistant with packs, a team of helpers that earn freedom, HRMS desks and AI health monitoring. Model quality
on real mail is proven offline (scripted planners, canned drafters), not yet on a live baseline.

## 6. The roadmap

From `docs/status-2026-10-15.md` ("After v1") and `docs/roadmap.md`:

| # | Item | Why it matters |
|---|---|---|
| 1 | Smarter matching: LLM rerank on top of the explained score | The staffing customer's first demo was referral to requirement matching |
| 2 | Members connect their own Gmail | Today an admin connects each mailbox; the "one personal agent per person" idea needs per-person consent |
| 3 | Live golden baseline | Scores from real models, kept as the bar, so a model swap can be judged |
| 4 | Understanding v2 switch (`UNDERSTAND_V2=on`) | One structured call instead of two; about 75% cheaper labelling; switch at 95% agreement |
| 5 | Ask Meera on HR numbers | The metric functions exist; the assistant does not call them yet |
| 6 | Restore drill in CI | Backups exist nightly; a tested restore does not |
| 7 | Scheduled performance summary and a "correction needed" alert | Managers asked for it |

## In this repo

- `README.md`: how to run it, what is real versus stubbed.
- `docs/architecture.md` sections 1, 5 and 11: the idea, the agent runtime, the security model.
- `docs/features.md` section 9: agents, levels, the sensitive floor, in user words.
- `docs/research-2026-10.md`: the outside evidence the design leans on.

## Exercises

1. **Say it.** Without reading, record a one-minute voice note explaining Post Room to a friend who runs a small
   recruiting firm. Then compare with section 1 and 3.1. What did you leave out?
2. **Find the floor.** Open `apps/api/src/agents/autonomy.rs` and read `blocker` (line 150). List, in your own
   words, the eight reasons a send goes to a person even at level 3.
3. **Trace a promise.** `docs/features.md` section 6 says "a draft with open gaps cannot be sent". Find the three
   places in code that enforce this (hint: `has_open_gaps` in `apps/api/src/replies.rs:230`, `check_gaps` in
   `apps/api/src/send_safety.rs:157`, and `hard_rules` in `apps/api/src/agents/critic.rs:65`). Why three and not one?

## Check yourself

1. What is the difference between level 2 and level 3?
2. Name three things on the sensitive floor that ask a person at every level.
3. Why are model names never shown to users?

<details>
<summary>Show answers</summary>

1. Level 2 replies only to the original sender, in the same thread, to a contact the workspace already wrote to. Level 3 may also reply to any participant of the conversation and to a first-time address. Both are held for ten minutes with a Stop button. See `blocker` in `apps/api/src/agents/autonomy.rs:150`.
2. Any three of: money, rates or prices; legal, contract or NDA words; security, passwords or OTPs; outgoing attachments; a new recipient outside the conversation; injection signals in the inbound mail; open `[NEEDS: ...]` gaps; anything in the agent's `never_do` list.
3. Two reasons. Product: the user should trust the helper by its behaviour, not by a brand; models change often and the user should not need to care. Engineering: model ids are configuration (`OPENROUTER_*`), so swapping a model must not change any UI text. The ledger maps tasks to plain names (`ai_ledger::task_label`) and an e2e test (`apps/web/e2e/no-model-names.spec.ts`) guards it.
</details>

**Next:** [01. Rust through this codebase](01-rust.md).
