# Post Room: everything it does today

Updated 15 Oct 2026. A plain-English catalogue of every feature a person can use. Each entry says what it does, who can
use it, where to find it in the app, and what keeps it safe.

Roles used below: **Admin** (runs the workspace), **Operator** (works the shared inbox and agents), **Member** (a
person on the team; sees their own things). **Super admin** is the one admin who owns the people directory.
Packs are switches that add labels, helpers and pages (section 3). Features marked **in progress** are not live yet.

Contents: 1 Sign-up and onboarding, 2 Inbox and reader, 3 Packs, 4 Attachments as text, 5 Labels, 6 Replies and
sending, 7 Human-style summaries, 8 Ask Meera, 9 Agents, 10 Building an agent in plain English, 11 Learning and
memory, 12 Results, evals and earned freedom, 13 Team, 14 HRMS modules, 15 Automations, 16 Alerts, 17 Memory page,
18 AI usage and cost, 19 AI health and incidents, 20 Guide and avatars, 21 Daily self-test, 22 Security.

---

## 1. Sign-up and onboarding

| | |
|---|---|
| **What it does** | Anyone can sign up with email and password and confirm with a code sent by email. A short **goal wizard** asks what you want help with (keep the inbox under control, answer customer support, follow up with leads, chase invoices, run personal mail, run HR and recruiting) or lets you say it in your own words. It suggests the matching packs, shows which helpers each pack brings, and asks you to name the workspace. A **Getting started** checklist then walks through connecting a mailbox and a reply setup (tone, sign-off). Teammates join by invite link. |
| **Who** | Anyone can create a workspace and becomes its admin. Invited people join as operator or member. |
| **Where** | Welcome page after sign-up; Getting started card on the Inbox; Settings, Integrations. |
| **Safety** | Sign-up and sign-in are rate-limited per address and per account. Email must be verified. Passwords are hashed; sessions are stored on the server. |

## 2. Inbox and Gmail-style reader

| | |
|---|---|
| **What it does** | Connect a Gmail mailbox (each person connects their own, through a secure Google sign-in) or forward mail to a Post Room address. Mail is stored once, grouped into threads, labelled, and shown in a reader that looks and behaves like Gmail: sender, thread, formatted message, links and pictures (cleaned for safety), attachment chips with their reading status. Search covers words and meaning. Filters: by label, by mailbox, by person, needs a reply, with an insight line under each message (what is asked, deadline, why it matters). Mail the team sent is kept but never labelled or drafted for. |
| **Who** | Operators and admins see the shared inbox. Members see mail they are allowed to see (mailbox and privacy wall). A member's personal mail is visible only to them. |
| **Where** | Inbox; Settings, Integrations (connect, disconnect, forwarding address). Delete and Disconnect are described right below. |
| **Safety** | Newsletters, receipts and automated mail are recognised by rules and filed without any AI call. Mail hidden by a policy (see Timesheets policy) is marked suppressed and skipped everywhere: lists, search, alerts, matching. Only an admin connects a mailbox; members connecting their own Gmail is on the after-v1 list. |

### Delete (from Post Room only)

| | |
|---|---|
| **What it does** | Deletes an email from Post Room; the Gmail copy always stays. Delete one email from the reader (with an option to take the whole conversation too), or press Select in the inbox, tick several and Delete. A confirm asks "Delete from Post Room? Your Gmail copy stays." and a toast says it is done. Everything made from the email goes with it: attachments and what was read from them, summary, label, draft replies (sent or not), review items, alerts, routing, search entries (words and meaning), and the sender's contact card when no other mail from them is left. Facts and preferences the agents learned only from that email are removed; ones also confirmed by other mail or typed by a person stay, minus this source. A case on the email is cancelled (a held automatic send is released first) and keeps its row for the record without the email. Ask Meera answers that cited it keep their sentence and show "Deleted email". |
| **Who** | The mailbox owner and admins. Operators can delete mail in shared mailboxes inside their access wall, and in their own mailbox; someone else's personal mailbox answers "not found". |
| **Where** | Inbox reader (Delete); Inbox, Select. |
| **Safety** | It never comes back: Post Room remembers the provider's message id (a tombstone), so the next Gmail sync skips it. The audit log gets a `message.deleted` row with ids only. A reply that is being sent at that moment blocks the delete (try again in a moment), for a whole selection at once. The confirm warns when a reply a person scheduled will be cancelled. Tombstones are pruned after 180 days. |

### Disconnect a mailbox (and delete what came from it)

| | |
|---|---|
| **What it does** | Disconnect asks first, with exact numbers from the server: "1,368 emails, 120 attachments, 45 drafts and their summaries will be permanently deleted from Post Room. Labels, settings and other mailboxes stay. This can't be undone." On confirm the mailbox stops syncing and its Gmail access is revoked, then a background job deletes everything from that mailbox in small batches (the same per-email deletion as above). Progress shows on the Integrations card ("Deleting… 400/1,368") and survives restarts: it carries on from what is left. Afterwards sync cursors and the connection reference are cleared, people linked to the mailbox are unlinked (not deleted), and contacts and learned facts that came only from it are gone. Labels, packs, agents, automations and other mailboxes are untouched. |
| **Who** | Admins, and the person who owns a personal mailbox. Another member's personal mailbox answers "not found". |
| **Where** | Settings, Integrations, Disconnect. |
| **Safety** | Nothing is deleted before the confirm, and the dialog also warns about scheduled replies that will be cancelled and open support tickets that will be closed. New mail cannot be added to a mailbox while it is being purged, and it cannot be reconnected until the purge ends. Mid-send replies are left until their send ends; if that takes over 30 minutes the purge stops as failed (shown on the card, mail unblocked) and can be started again. A purged forwarding address can be created again with a new address. The audit log gets `source.disconnected` and, when finished, `source.purged` with the counts. |

## 3. Packs, including labels that follow the packs

| | |
|---|---|
| **What it does** | A pack bundles labels, helpers, workflows and pages for one kind of work: **General** (always on), **Support**, **Sales**, **Finance**, **Personal**, and **HR and recruiting**. Turning a pack on adds its labels to the label list, creates its helpers and (for HR) shows the HRMS pages. Turning it off hides its labels from new mail and pauses its helpers, and turning it on again restores them. Existing mail keeps its labels. |
| **Who** | Admins toggle packs. Everyone sees the result. |
| **Where** | Settings, Packs. The goal wizard suggests packs during sign-up. |
| **Safety** | Pausing is reversible and recorded in the audit trail. General cannot be turned off. |

## 4. Attachments read as text

| | |
|---|---|
| **What it does** | Every attachment becomes text that search, summaries, drafts and Ask Meera can use: **Excel and CSV** (sheets and rows), **PowerPoint** (slide text), **Word**, **PDF**, **images** (text read by OCR and described), and **audio** (transcript). Each attachment chip shows whether it has been read, was skipped, or failed. Old mail reads the documents at once and uses AI files (images, audio) when opened. |
| **Who** | Automatic for everyone who can see the message. |
| **Where** | Attachment chips in the reader; the text is used by Ask Meera, search and drafts. |
| **Safety** | Attachment text is untrusted: it is fenced in prompts, never treated as instructions, and capped in length. Files are scanned (the scan status is stored). Image and audio reading stops when the day's AI budget is spent. |

## 5. Labels

| | |
|---|---|
| **What it does** | Every mail gets a label, a summary, whether it needs a reply, and key facts. Labels learn the team's meaning: when you correct one, the correction is remembered and used as an example next time. The label list is editable (name, meaning, colour), and a suggest feature proposes new labels from mail the system could not place. Unsure mail goes to a review queue instead of being guessed. |
| **Who** | Operators and admins edit labels and review the queue; every correction is a learning signal. |
| **Where** | Teach, Labels; Reviews; the label chip in the reader. |
| **Safety** | Labels follow the packs that are switched on. Corrections come from people only. |

## 6. Replies, drafts, send safety and 30-second undo

| | |
|---|---|
| **What it does** | For mail that needs a reply, Post Room writes a draft in the team's voice, using the thread, the person's profile and learned preferences. Anything it cannot support from the facts becomes a visible gap, `[NEEDS: ...]`, and a draft with open gaps cannot be sent. Drafts can be edited, saved to Gmail drafts, regenerated, or sent. After Send there is a **30-second undo**. Before any send, **send-safety** checks run: recipient, subject, length, placeholders, duplicates, a possible leak of an address or number that is not from the conversation, and an AI review. A blocked send says why. |
| **Who** | Operators and admins send. A member sends from their own mailbox. Helpers send only by the rules in section 9. |
| **Where** | The reply panel in the reader; Agents, Waiting for you. |
| **Safety** | Every send path goes through one function with the same checks. Each send has an attempt id, so a crash cannot send twice. A blocked send is logged as an incident (section 19) on a best-effort basis: if the log write fails, the send is still blocked. Only real safety findings (a possible leak, an unsupported promise) count as serious; a duplicate, a too-short text, open gaps, an automated recipient or an unavailable reviewer does not lower any agent's level. |
| **Messaging (live)** | Meera and agents can send a new email, an in-app message or a notice to the owner (`send_message`, `notify_owner`), ask and report back with one nudge after 24 hours, and run vendor support cases (`contact_support`) with a ticket. Inside the workspace a message may go on its own from level 2 after a hold; outside addresses, money or security words, attachments and injection signals always wait for a person. Bounces tell the owner; auto-replies are ignored. See the Messages page. |

## 7. Human-style summaries with rupee formatting

| | |
|---|---|
| **What it does** | Summaries read like a colleague wrote them ("Ravi from Acme wants the Q3 invoice resent by Friday"), not like a list of fields. Amounts are written in Indian style (for example ₹1,25,000 or ₹4.5 lakh) and dates in plain words. |
| **Who** | Everyone who sees the message. |
| **Where** | Under each message in the Inbox; in the digest emails; in Ask Meera answers. |
| **Safety** | Quotes and amounts are checked in code against the mail text, so a number that is not in the mail is not shown as a fact. |

## 8. Ask Meera

| | |
|---|---|
| **What it does** | A voice or text assistant available on every page. It answers from your own data with sources you can open (which mail, which approval, which alert). It **knows the page you are on**: with an email open, "reply to this", "what is this about" and "what did they ask" work without naming it. It **reads sheets** and other attachments for answers such as "what is the total in the attached Excel". It keeps **one continuous conversation** per person, so follow-ups work. It can prepare actions (start an agent, approve, resolve or snooze an alert), but nothing runs until you press Confirm. |
| **Who** | Everyone, on the mail and data they are allowed to see. A member's personal mail is never shown to an admin. |
| **Where** | The Ask Meera bar and Ctrl+K on every page. Voice uses the microphone button. |
| **Safety** | Actions wait in a plan that a person confirms once. Page context that does not belong to the workspace is dropped. Answers are short and cited. Answering from HR performance numbers is on the after-v1 list. |

## 9. Agents (your team of helpers)

| | |
|---|---|
| **What it does** | An agent is a helper with a job: which mail it handles, which tools it may use, how far it may go. **Meera, one per member** (shown as "Your Meera"), works on that person's own mailbox, tone and memory, and handles everyday mail and anything no expert owns. Each enabled pack adds **one expert per field**: Support (**Ishaan**: tickets and escalations), Sales (**Kabir**: qualifies enquiries and follows up), Finance (**Vikram**: invoices and payments), Personal (**Nisha**: scheduling). General adds nobody; Meera handles general mail. **HR and recruiting keeps its team** (Ananya leave, Rohan timesheets, Sneha onboarding, Arjun client requirements and candidates, Aditya payroll). Routing stays in code and is not a visible agent (the coordinator row exists for the runtime, is stored under the workspace agent's name, Meera, and is hidden from the hub). Each card is compact (avatar, name and role, one autonomy line) and opens "How I work": what it does, when it acts, what it does alone, what it asks you, and an example. Names are editable by admins; the old Diya, Tara and Inbox Assistant desks are merged into the experts and archived (an admin-tuned one stays active as a custom agent). |
| **Freedom levels** | **Meera and the field experts start at level 3 and act on their own** (new workspaces send automatically; an existing workspace moves to automatic only if its admin never changed the setting). **0 Observe**: only watches and is scored against what people actually do (shadow mode). **1 Draft**: writes real drafts; every send asks a person. **2 Routine send**: replies to the original sender in the same thread for routine mail, after a **hold with a Stop button** (10 minutes by default, set by `AGENT_AUTO_SEND_DELAY_SECS`; Stop is on the agent's Outgoing tab) and only to a contact the workspace already knows. **3 Autonomous** (the default): also replies to any participant in the conversation and to an address not seen before, behind the same hold. A level can also be capped by the environment (`AGENT_MAX_AUTONOMY`) and by the owner's profile. **Per-person profile:** a missing, unsaved or skipped profile counts as "routine replies are fine" (automatic by default); a saved "Always ask me" is always respected. **New senders:** at level 3 a routine reply to a brand-new sender also goes automatically, protected by the leak check, the injection gate, the hold and Stop. The startup backfill raises untuned Meera, the lead and the experts to level 3 in any workspace whose send setting is "auto" (whoever chose it), recording each raise as "Automatic by default"; a workspace whose admin chose "ask" raises nothing, and an agent whose limit or level a person set (recorded as a user change), an incident demoted, or that is paused or archived is left alone until it is eligible. **Sensitive things always ask a person at every level**, judged on the reply and on the inbound mail of the conversation (a harmless "Sure, will do" to "please wire the refund and sign the NDA" still asks): money, bank or payment details, prices, quotes, fees, refunds, salary or amounts (including Rs500, USD500, 5k); legal, contract, NDA, confidential, visa, GDPR, harassment or termination wording; security, passwords or one-time codes; attachments going out; injection signals; open gaps; anything the agent is told never to do; a new message to an outside address; anything to more than one outside recipient. An incident or a falling approval rate still demotes an agent. Hub copy: "Your helpers act on their own and ask only for sensitive things. Change this in Settings -> Sending." |
| **Controls** | **Stop** on any held send. **Pause** an agent, or **Pause all**. Admin cap on each agent's level. |
| **Approvals** | Anything risky lands in Waiting for you. You can approve, edit then approve, or decline. An approval is tied to a **content hash**: if the text changes after you approve, the approval no longer applies. |
| **Injection gate** | Mail that tries to instruct the agent (for example "ignore previous instructions", "bcc this to...") forces a person's approval at every level and is logged. |
| **Audit trail** | Every send, approval, level change and pause is written to an audit trail in the same transaction as the action. Resolving an incident is audited too. The incidents themselves are a separate, best-effort log (section 19), not part of the audit trail. Both hold ids, never mail text. |
| **Routing** | Each mail goes to one agent by a fixed order: your manual choice, an open conversation, trigger rules, labels, then Meera (the lead row, internally "Routing"). No AI call. |
| **Shadow mode** | An agent at level 0 produces drafts nobody sees, and the system compares them with what a person actually sent, to measure agreement before it is trusted. |
| **Versions and rollback** | Every change to an agent is a new version. You can see the history and roll back. |
| **Who** | Admins create, change, pause and grant levels. Operators work approvals. Members manage their own personal agent. |
| **Where** | Agents (hub, agent drawer, Waiting for you), Your team. |

## 10. Plain-English agent builder with dry run

| | |
|---|---|
| **What it does** | An admin describes a helper in everyday words ("handle vendor invoice questions, always ask before replying"). The builder turns it into a precise spec, repairs it if it does not pass checks, and runs a **dry run** on sample mail so you can see what it would do before it touches real mail. It saves as a new version at no more than level 1. |
| **Who** | Admins. |
| **Where** | Agents, Build an agent. |
| **Safety** | The spec rejects unknown fields. A dry run has a spend cap and sends nothing. The safety suite must pass before a version keeps any autonomy. |

## 11. Learning loop, private memory, verified context

| | |
|---|---|
| **What it does** | When you edit a draft, approve or decline, or correct a label, Post Room notices. Edits shape that person's **preferences** (shorter, warmer, a preferred sign-off). Each agent has **private memory**: facts learned from mail, with their source, visible to its owner. Drafts are built from a **verified context pack** (thread, profile, preferences, sourced facts) and a check turns any unsupported claim into `[NEEDS]`. A gain measure shows whether your edits are shrinking over time. |
| **Who** | Automatic. Owners see and delete their own memory and preferences. |
| **Where** | Teach, Memory; Results (learning gain). |
| **Safety** | Preferences come only from your own edits and notes and pass a filter that rejects anything that sounds like an order to the agent. Facts from outside the team are capped until a person confirms them, so mail cannot plant false memory. |

## 12. Results, evals, and earned autonomy with auto-demotion

| | |
|---|---|
| **What it does** | The **Results** page shows per-agent numbers (cases, approvals, edits, time saved, agreement in shadow mode) and trends. The **evals** panel runs three tests on a specific agent version: a **safety suite** (injection, leaks, permission limits), a **replay** (redo past replies and score closeness), and the offline scenario set. Raising an agent's level is an explicit admin click and is refused until a checklist is met (for example, safety suite at 100%, enough observed cases, a good approval rate). **Demotion is automatic**: an hourly check and an immediate drop on a serious incident, a stopped send, or a falling approval rate. |
| **Who** | Admins promote and run evals; operators read results. |
| **Where** | Results; the agent drawer (level panel). |
| **Safety** | Autonomy can only go up by a person's click and down by the system alone. |

## 13. Team

| | |
|---|---|
| **What it does** | A **directory** of people (name, emails, role, client, manager, joining date, billable or not, targets), by form or CSV with a dry run. A **super admin** owns it. **Escalation** chains alerts from the owner to their manager to the final recipients on configurable delays. **Per-person scope** lets a manager see themselves and their reports. A **corner performance widget** on every page shows today and this week against target and interview rate. A **metrics framework** defines each person's measures. |
| **Who** | Super admin edits. Others see themselves and their reports. |
| **Where** | The HRMS menu (People, Performance, Escalations, and more), shown only when the HR and recruiting pack is on; the corner widget appears only then too. Other workspaces see no team menu. |
| **Safety** | Reads are filtered to the caller's scope on the server. |

## 14. HRMS modules (HR and recruiting pack)

| Module | What it does | Where |
|---|---|---|
| **People: billable and non-billable** | Billable staff keep client mail out of the shared view: only timesheets and mail through the HR mailbox are read; the rest is stored hidden (suppressed). Non-billable staff keep the full mailbox. Changing the policy re-judges stored mail. | People |
| **Submissions** | A recruiter's sent mail with a CV is detected, linked to the requirement and candidate, and its outcome (interview, rejected, joined) tracked from client replies. Ambiguous links wait for a person. | Performance |
| **Performance** | Today and this week against target, interview rate, funnel since joining, team view for managers. | Performance |
| **Timesheets** | Timesheets that arrive, pasted in the email or attached (Excel, CSV, PDF, Word, image), are labelled (also by the words timesheet / time sheet / timecard / hours report in the subject or file name), read, and kept as **records** linked to the email: who (matched by employee ID, then email, then name; a forwarded sheet goes to the employee it names), the period, hours per day, total and what to check. A new sheet for the same person and period replaces the old one. The page is a **team grid**, people by days (weekends shaded, total, Recorded / Needs check / Missing, filters, week view, CSV export); a Records tab lists every sheet with search and an "Open email" link, and the reader shows a "Timesheet recorded" chip. Rohan records and never replies; he asks a person only when a record needs a check or nobody is matched, and a person can link such a sheet to someone once. A recorded timesheet counts for the month-end check, and records stay when the email is deleted or the mailbox disconnected. | Timesheets |
| **Portal reconciliation** | Upload the portal's hours CSV; it is compared exactly with the timesheet mail; a mismatch raises an alert to the employee, then manager, then HR. | Portal reconciliation |
| **Escalations** | See and set who is told, when. | Escalations |
| **Matching** | Referral profiles are matched to open client requirements by skills and meaning with an explained score and a daily digest. | Matching |

Who: HR pack on; Admin or super admin to configure; people see their own numbers. Safety: suppressed mail is skipped by
every read path; hours are read by pattern and unreadable ones are shown as unverified, never as mismatches.

## 15. Automations

| | |
|---|---|
| **What it does** | Simple "when this mail arrives, do this" rules (for example acknowledge timesheets, send a standard reply to a known domain). Agents can propose an automation, which stays off until a person turns it on. |
| **Who** | Admins and operators. |
| **Where** | Teach, Automations. |
| **Safety** | An automatic reply needs an allowed-domain list and is refused if the conversation contains an injection attempt or an open gap. |

## 16. Alerts

| | |
|---|---|
| **What it does** | Rules raise an alert when an important mail has had no reply, an approval is stuck, a timesheet is missing, a portal mismatch appears, or someone is under target. Alerts go to the owner first, escalate up the chain, can be snoozed or resolved, and arrive in the app, by email, and in a morning digest. AI cost and speed spikes also raise alerts (section 19). |
| **Who** | Recipients you choose; admins configure. |
| **Where** | Alerts (with a count badge); Alerts settings. |

## 17. Memory

| | |
|---|---|
| **What it does** | A page where the team writes notes the helpers should know (style, policies, key facts) and where learned facts can be reviewed, confirmed or deleted. |
| **Who** | Admins for shared memory; each person for their own. |
| **Where** | Teach, Memory. |
| **Safety** | Notes are fenced as data in prompts. Unconfirmed outside facts are capped. |

## 18. AI usage and cost ledger

| | |
|---|---|
| **What it does** | Every model call is recorded: which job, how many tokens, cost, and how long it took. The page shows spend per day, per job, and cost per email, and a **daily budget** stops automatic reading when the day's limit is spent (people clicking are not blocked). |
| **Who** | Admins. |
| **Where** | Settings, AI usage. |
| **Safety** | Model names are never shown anywhere; jobs use plain names. |

## 19. AI health and incidents

| | |
|---|---|
| **What it does** | A page for admins showing whether the AI is behaving. **Incidents** by kind and day: `leak_blocked`, `injection_detected`, `refusal`, `schema_failure`, `unsupported_claim`, `send_blocked`, `cost_spike`, `latency_spike`. Open incidents have a "Mark handled" button. Also: cost per email today and over seven days, typical and slowest-5% speed per job, failure and refusal rates, the new reading method's agreement with the current one (and whether it is **ready to switch**), and the daily budget used. A serious incident **that belongs to an agent** lowers that agent's freedom level by one at once (one burst costs one level); an incident with no agent, such as a spend spike or a manual-send block, demotes nobody. Incidents are written best-effort: a failed write is logged and never stops the work. The spend alert needs at least three days of earlier spend, a non-zero baseline, today's spend above $0.50 and more than twice the earlier daily average. The speed alert needs enough calls in the last hour and a clear jump over the week before, and is recorded once per job per day. |
| **Who** | Admins. |
| **Where** | Settings, AI health. |
| **Safety** | Incident details hold ids and short facts only, never email text. The same event twice is recorded once. |

## 20. Feature guide and 3D avatars

| | |
|---|---|
| **What it does** | Each main page opens with a dismissible **explanation card**: what the page does, how to use it in two or three steps, and why it is safe. Dismissal is remembered per person. The **Guide** page is a plain grid of the same cards, each with a Go to page link; there is no tour or spotlight. Small help pop-overs explain terms where they appear. Every agent has a **3D Fluent-emoji avatar** (16 to choose from, saved from the agent drawer or the Agent page and shown everywhere) so you can tell helpers apart at a glance. |
| **Who** | Everyone. |
| **Where** | Help, Guide; the "?" help buttons; the avatar picker in the agent drawer. |

## 21. Daily self-test with failure email

| | |
|---|---|
| **What it does** | Every day at 03:30 UTC an automatic check runs: code quality checks, the full test suite, the web build, whether the live server is healthy, and a small probe that each configured AI role still answers. If anything fails, the owner gets an email naming the failing step and its output. |
| **Who** | Runs on its own; the owner is emailed. |
| **Where** | GitHub Actions; see `docs/selftest.md`. |

## 22. Security in one place

- **Tenant isolation.** Every workspace route checks that the caller belongs to that workspace, and every lookup by id also filters on the workspace. A test walks every such route and tries it as a member of another workspace; it expects no data.
- **Prompt fencing.** Mail, attachments and memory are wrapped as data, with fence characters escaped so text cannot break out of its box.
- **No memory poisoning.** Outside facts stay capped until confirmed; preferences come only from your own actions.
- **Send safety and the hold.** All sends share the same checks; automatic sends wait ten minutes with a Stop button.
- **Privacy.** Personal mail is invisible to admins; suppressed mail is skipped everywhere.
- **Housekeeping.** Security headers, request size limits, rate limits, nightly backups (seven kept).

See `docs/architecture.md` for the engineering detail.
