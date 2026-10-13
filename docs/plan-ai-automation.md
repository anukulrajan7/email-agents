# Plan: AI automation with retrieval (RAG)

Status: proposal for review · 14 Oct 2026 · supersedes ad-hoc model/prompt changes

## 1. What went wrong so far (from production data, 101 emails)

| Observation | Cause | Lesson |
|---|---|---|
| 86/101 emails flagged "Needs review" | Policy forced review for every "Other", however confident | Automation rules must be per decision, with measured thresholds |
| 86/101 labelled "Other" | Labels were judged from a 1-line snippet before full bodies arrived; the model has no idea what *this team* means by each label | The model needs **this team's examples**, not just label descriptions |
| Matching uses keyword lists | "Spring" vs "Spring Boot", "ReactJS" vs "React", synonyms in CVs are missed | Recall needs **semantic** search, not regexes |
| Drafts sound generic | Prompt only has a policy template and a few memories | Drafts should be grounded in **the team's own past replies** |

Each fix so far was local. The common missing piece is **retrieval**: giving every AI step the most relevant
*evidence from this workspace's own history* (past emails, human corrections, sent replies, CVs, JDs).

## 2. The core design: one retrieval layer, used by every step

```
                 ┌──────────── Knowledge store (per workspace, SQLite) ─────────────┐
email/attachment │ chunks(text, kind, refs)  ── FTS5 (keywords, BM25)              │
  ──► chunk ──►  │ embeddings(vector)        ── sqlite-vec (semantic, cosine)      │
      embed      │ facts: labels, corrections, approved replies, records, memories │
                 └───────────────▲────────────────────────────────────────────────┘
                                 │ hybrid search = BM25 ⊕ vector (reciprocal rank fusion)
                                 │ + filters: workspace, kind, date, verified-only
   ┌───────────┬─────────────────┼──────────────┬──────────────┬──────────────┐
 Label       Draft          Extract/Match     Ask agent     Reminders
 (few-shot)  (style+facts)  (semantic recall) (cited Q&A)   (rules, no RAG)
```

- **Embeddings run locally** (`fastembed`, model `bge-small-en-v1.5`, 384-dim, ONNX on CPU). Email text never
  leaves the server just to be embedded. It costs nothing per call, and about 3 ms per chunk on the VPS.
- **Vectors live in SQLite** through the `sqlite-vec` extension. That's the same file and the same backups, with no
  new database to run, consistent with the "no vector DB service" constraint.
- **Hybrid search:** keywords (FTS5) catch exact names, IDs and skills; vectors catch meaning ("hours worked" ≈
  "timesheet"). The two are merged by rank. All queries are filtered by `workspace_id` (tenant isolation).
- **Only trusted, verified context** is used as *examples*: human-approved labels, sent or approved replies, and
  verified records. Raw email text goes in as *evidence*, clearly marked as untrusted data.

## 3. Where RAG is used and where it is deliberately not

| Step | Approach | Why |
|---|---|---|
| **Labelling** | RAG few-shot: retrieve the 5 most similar past emails **with human-confirmed labels** and show them to the model ("your team labelled these as…"), plus sender history | Learns each team's own meaning of a label with no fine-tuning. Every correction improves the next email immediately |
| **Drafting replies** | RAG over **this team's sent/approved replies** to similar emails, plus memories and the sender's thread | Replies match the team's tone and standard answers, and the facts come from real history |
| **Matching** | **Vector recall** (JD ↔ CV embeddings) to find candidates, then the existing **explainable score**, then a short LLM rerank with reasons | Semantic recall finds synonyms and related skills; the score keeps decisions auditable |
| **Extraction** (requirement/candidate fields) | **No RAG.** Schema extraction from the document itself; a skill taxonomy for normalisation | The answer is in the document; retrieval adds noise |
| **Ask agent** (Shrikant's Q&A) | **RAG for "what/why" questions** (search emails, CVs, JDs with citations) + **tools for numbers** (SQL views: submissions per recruiter, interview rate) | RAG can't count reliably; metrics must be exact queries |
| **Reminders / SLAs** | **No RAG.** Deterministic rules on thread state; the LLM only judges "needs a reply?" (already stored) | Deadlines and escalations must be predictable |
| **Portal reconciliation** | **No RAG.** Exact joins between CSV rows and extracted records | Mismatches must be exact |

## 4. Automation levels (what runs without a human)

| Decision | Level | Rule |
|---|---|---|
| Label an email | **Auto** | Auto-approved when confidence ≥ 0.80 (calibrated, see §6). Below that, the stronger model (Kimi K2.6) re-checks; still below → human review |
| Run a workflow (review item + draft) | **Auto** | Always, idempotent |
| Save the reply into Gmail Drafts | **Auto** | For workflow drafts on connected mailboxes; the person only opens Gmail and presses Send |
| Send a reply | **Human** (default) / **opt-in auto** | Opt-in per label with allow-list + daily cap + audit; never for clients or money until pilot evidence |
| Extract requirement / candidate | **Auto** | Records marked unverified until a person opens them; scores show "check details" |
| Shortlist a match | **Human** | The AI ranks; a recruiter decides |
| Remind / escalate | **Auto** | Rules: important + no reply in N hours → owner → manager → HR |

The **feedback loop** is what makes this improve on its own. Every approve, correction, edit or shortlist is stored and
becomes retrievable evidence for the next decision. No retraining job is needed.

## 5. Model routing

| Task | Model | Notes |
|---|---|---|
| Labelling, extraction, learning | `qwen/qwen3.8-flash` | Cheap and fast; reasoning off |
| Escalation (unsure / error) | `moonshotai/kimi-k2.6` | Sonnet-level; only for the uncertain minority |
| Drafts, match reasons, Ask agent | `moonshotai/kimi-k2.6` | Quality matters more than cost here |
| Embeddings | `baai/bge-m3` (1024-d) via OpenRouter | Decided over a local model: simpler to run; same no-retention routing as the other calls |

All calls use `data_collection: deny` routing and JSON schemas, are validated in Rust, and record the model used.

## 6. Measure before trusting (evaluation)

- **Golden set:** 100 real emails from the workspace, labelled by Anukul/Shrikant (the UI already supports this
  through approve/correct). We measure accuracy per model, with and without RAG examples, before changing thresholds.
- **Calibration:** pick the auto-approve threshold where precision ≥ 95% on the golden set.
- **Live metrics:** auto-approved %, human-correction rate per label, escalation rate, draft edit distance,
  match shortlist rate, cost per 1,000 emails. These go on an internal "AI quality" page.

## 7. Build order

| Step | Scope | Done when |
|---|---|---|
| **R0: Quick fixes** (ready) | Confident labels auto-approved (incl. Other); Qwen → Kimi escalation; HTML email display (sanitised, links + images in a sealed frame) | Review queue only holds genuinely unsure emails; emails render like Gmail |
| **R1: Knowledge store** (built, wave 2) | Chunking (email body + attachment text + approved/sent replies), `bge-m3` embeddings via OpenRouter (no-retention routing), sqlite-vec, hybrid search API (`GET /search`), backfill existing mail | `search("java spring pune")` returns the right emails/CVs; latency is now bounded by one embeddings call (~0.2–0.5 s), not < 100 ms |
| **R2: RAG labelling + golden set** (few-shot built; evaluation open) | Few-shot examples (up to 5) from confirmed, human-set, or ≥ 0.9-confidence non-`other` labels; evaluation page; calibrated threshold | Measured accuracy gain on the golden set; review rate falls |
| **R3: RAG drafting + auto Gmail drafts** (retrieval built) | Retrieve similar sent replies (up to 3, done); auto-save workflow drafts to Gmail | Draft edit distance falls; drafts waiting in Gmail |
| **R4: Semantic matching** | Vector recall + score + LLM rerank with reasons; daily digest email | Synonym cases matched; digest each morning |
| **R5: Reminders + Ask agent** | SLA rules and escalation; Ask page (RAG + metric tools, cited answers) | "Who hasn't replied to Capgemini since Monday?" answered with links |

## 8. Decisions needed

1. **Embeddings:** local model on the VPS (recommended: private, free) vs an embeddings API (simpler, but email text leaves the server).
2. **Golden set:** can Shrikant/Anukul label ~100 real emails once (about 20 minutes in the UI)? Without it, thresholds are guesses.
3. **Auto-send:** keep human-only for now (recommended), or allow opt-in auto-send for specific labels (e.g. acknowledgements)?
