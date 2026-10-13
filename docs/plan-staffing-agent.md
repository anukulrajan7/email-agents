# Plan: Staffing operations agent (Shrikant's requirements)

Status: proposal · 14 Oct 2026 · builds on Post Room (labels, memory, Nango Gmail, workflows, audit)

## 1. What was asked, in our words

| # | Requirement (from Shrikant) | What it means in the product |
|---|---|---|
| R1 | Leave + candidate referral handling | ✅ Shipped: workflows on `leave` / `candidate_referral` labels |
| R2 | AI reminders when an important email is missed; trigger events | SLA rules: "important + no reply in N hours → remind owner → escalate to manager" |
| R3 | Each person's performance in a corner widget: day, week, target vs achieved | Per-recruiter metrics from **submissions**, with targets |
| R4 | Two employee types: **billable** (at client, client email; send timesheets/important mail to our HR inbox monthly) and **non-billable** (internal, full mailbox access) | People directory with type, client, manager. Billable → data only via HR mailbox. Non-billable → their own mailbox connected |
| R5 | Match email data with portal data; alerts for corrections/approvals at each level | Reconciliation: timesheet hours in email vs portal → mismatch → employee → manager → HR (escalation levels) |
| R6 | Read all emails **and attachments** first; outputs later | Attachment text extraction (PDF, DOCX, XLSX) + structured extraction per label |
| R7 | Agent reads all team members' company email | Many mailboxes per workspace, each mapped to a person |
| R8 | Ask questions about all the data; monitor submissions + quality since joining date | "Ask" page: an AI agent with safe, read-only tools over our data |
| R9 | Match friend-referral profiles to daily client requirements (Capgemini, TCS, …) from recruiter/client emails | Requirement ↔ candidate matching with a daily digest |

## 2. The core idea: emails become records

Today every email gets **a label**. Next, each label can have an **extraction schema**. The AI fills in typed fields
(validated JSON) and a person verifies them, the same "AI proposes, human approves" loop we already use.

```
email (+ attachment text) → label → extraction (typed JSON) → record (requirement / candidate / submission /
timesheet / leave) → rules (reminders, matching, reconciliation) → dashboards + Ask
```

| Label | Extracted record | Key fields |
|---|---|---|
| `client_requirement` | Requirement | client, role, must-have skills, experience range, location, positions, budget, deadline, recruiter |
| `candidate_referral` / `candidate_profile` | Candidate | name, skills, years, location, notice period, current/expected CTC, resume text, referred by |
| `candidate_submission` (recruiter → client) | Submission | candidate, requirement, client, recruiter (sender), date |
| `submission_update` (client → us) | Submission status | interview / rejected / selected / joined, date |
| `timesheet` | Timesheet | employee, period, hours, approver, client |
| `leave` | Leave | employee, dates, type |

Every record links back to its source email and attachment ("why does the dashboard say this?"), and is
workspace-scoped and audited.

## 3. Building blocks (new)

1. **People directory:** `people` (name, emails, `billable`/`non_billable`, client, manager → levels, joining date,
   role: recruiter/employee/manager, daily and weekly submission targets). Mailboxes and senders map to people.
2. **Many mailboxes:** several Nango Gmail connections per workspace, each owned by a person (already supported by
   `email_sources`). Billable staff add no mailbox; their mail arrives through the HR inbox.
3. **Attachment reading:** download through Nango's proxy (Gmail `attachments.get`) or from forwarded mail. Extract
   text from PDF (`pdf-extract`), DOCX (zip + XML) and XLSX (`calamine`). Type allow-list, 10 MB cap, text stored
   capped, binary not kept. A malware scanner (ClamAV sidecar) comes before any download button.
4. **Extraction engine:** per-label JSON schema → OpenRouter `json_schema` (Qwen for volume, Kimi for hard
   cases) → validated in Rust → `extractions` (draft) → verified → typed tables.
5. **Rules and reminders engine:** the timesheet workflow generalized into `rules`. *When* (label, field condition,
   or "no reply after N hours") → *then* (create review item / reminder / notify by email / escalate to the
   next manager level / webhook). Reply detection uses the Gmail thread: a later message from our domain in the
   same thread counts as handled. A scheduled job checks SLAs every 15 minutes.
6. **Matching:** requirement ↔ candidate. Hard filters (skills overlap, experience band, location), then FTS5
   recall, then an LLM re-rank with a written reason ("3/4 must-have skills, 6 yrs vs 5–8, Pune ✓, notice
   60d ✗"). A daily digest email per recruiter, and one-click "submit" creates a draft email (never auto-sent).
7. **Metrics and dashboards:** submissions per recruiter per day/week vs target, plus quality funnel
   (submitted → interview → selected → joined) since joining date. A corner widget shows "my day / my week / target".
8. **Portal reconciliation:** first a CSV import (timesheets, submissions) from the portal, later an API
   connector. Mismatches become alerts routed by level (employee → manager → HR).
9. **Ask (Q&A agent):** a chat page. The model gets **read-only tools only**: `metrics(person, range)`,
   `search_emails(q)`, `list_requirements(filters)`, `candidates_for(requirement)`, `submissions(filters)`.
   No raw SQL from the model. Every answer cites the records and emails it used.

## 4. Phases

| Phase | Scope | Demo at the end |
|---|---|---|
| **P1: Foundations** | People directory + billable/non-billable + manager levels · multi-mailbox per person · attachment text (PDF/DOCX/XLSX) · extraction engine with Requirement, Candidate, Submission, Timesheet schemas + verify UI · reminders v1 (important + no reply → remind → escalate; email digest) | "A TCS requirement email and a referral CV arrive; both become records; an unanswered client email escalates to the manager after 4 hours" |
| **P2: Matching + performance** | Requirement ↔ candidate matching + daily digest · submission tracking + status updates · performance dashboard + corner widget (day/week, target vs achieved, quality funnel) | "Each morning, every recruiter sees new matches and their own target progress" |
| **P3: Reconciliation + Ask** | Portal CSV import → timesheet/submission reconciliation → level-wise alerts · Ask agent with cited answers | "Ask: *How many submissions did Priya do since joining, and what % got interviews?*" |

## 5. Risks to manage up front

- **Reading employees' mailboxes** needs a written company policy and employee notice. India's DPDP Act requires a
  stated purpose and limited retention. Personal or unrelated mail must be excluded: label `other` stays
  unstored beyond metadata.
- **Google restricted scopes** (`gmail.readonly`) need Google verification before connecting mailboxes outside test
  users. A Google Workspace admin (domain-wide) setup is the alternative to per-person OAuth.
- **Accuracy:** every extracted number shown on a dashboard must be human-verifiable. Records keep a link to the source.
- **Cost:** Qwen Flash at roughly $0.15/M input. 1,000 emails/day ≈ a few dollars a month. Kimi only for re-rank and drafts.

## 6. Decisions needed from Shrikant / Anukul

1. Mailbox access: each person clicks **Connect Gmail**, or a Google Workspace **admin** connects all mailboxes once?
   (And is the company on Google Workspace or Microsoft 365?)
2. Portal: which system holds timesheets/submissions, and can we get an **API** or a **CSV export**?
3. Definitions: what counts as a **submission** and as **quality** (interview rate? selection rate?), and where do
   **targets** come from (per recruiter, per day/week)?
4. First demo: which single flow should Shrikant see working first?
