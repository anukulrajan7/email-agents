# Building better agents in Post Room

A short, practical checklist for anyone adding or changing an AI call, a workflow or an agent in `apps/api`. Each rule cites its source: [A1] building effective agents, [A2] context engineering, [A3] agent evals, [O] the OpenAI guide, [G] Google design patterns, [OR] OpenRouter docs, [DS] DeepSeek caching. URLs are at the end. Background and numbers: docs/ai-engineering-review.md.

## 1. Choose the simplest shape
- **Single call first, then a workflow, then an agent.** A workflow is a fixed code path (understand → draft → approval). An agent is a planner loop. Use the loop only when you can't predict the steps in advance (multi-day requirement cases, follow-ups, escalations). [A1][G]
- **Routine replies are a workflow** (`mode: "workflow"`). A planner turn costs 4–5× a draft and adds a job round trip each time. [A1]
- **Route with code, not a model.** Use labels, intent, sender role, domains and attachment types (`agent_registry::routing`). [A1 routing pattern]
- **Evaluator-optimizer only with clear criteria.** The critic and send judge ask typed yes/no questions; they never rewrite free text. [A1]

## 2. Understand once, act from typed facts
- **One understanding call per email** (`understand.v1`) produces label, priority, needs_reply, asks with quotes, commitments, deadlines, entities, sentiment and facts. Everything downstream reads the stored result; nobody re-reads the body. [A2 just-in-time]
- **Quarantine the planner.** It receives typed fields and datamarked excerpts, never raw bodies. Tool results follow the same rule. [Spotlighting][CaMeL]
- **Verify quotes in code.** Drop any ask or commitment whose quote isn't in the source, and log an incident. [A3 code graders]

## 3. Build every prompt from a context pack
- **Order:** static rules → workspace-stable blocks (labels, style, persona, tool specs) → case-stable blocks → per-email data, with the untrusted email last and a one-line recap after it. This order gives cache hits and keeps attention on the right things. [A2][DS]
- **Trusted text only in the system message:** rules, pack playbook, profile, guard-checked style preferences. Anything derived from an email (summaries, asks, learned facts) goes in the user message, fenced, with provenance (`[staff]`, `[learned]`, `[stats]`, `[email]`). [A2]
- **Fence untrusted text through `ai::fence`** (move to a per-call nonce tag). Send the new part of the email only; quoted history goes in as the thread digest. [Spotlighting]
- **Every block has a char budget enforced in code.** Retrieve the top-k for this email; don't preload everything. [A2 attention budget]
- **Prefer a few canonical examples over long rule lists.** The team's own labelled emails and past replies work best. [A2]

## 4. Tools (the agent-computer interface)
- **Few, non-overlapping tools per agent** (the spec allowlist), each with a clear description, enums and a risk tier. [A1][A2][O]
- **Poka-yoke.** Recipients come from code (thread participants only). `send_email` takes a draft id, not free text. Arguments the model can't get wrong. [A1]
- **Return meaningful errors** ("blocked: new recipient needs approval") so the planner changes course instead of repeating itself. [OR tool calling]
- **Risk tiers drive approval:** read → auto, draft → queue, send/forward/new recipient/attachment out → a person. [O]

## 5. Guardrails are layered
- Hard rules in code (`critic::hard_rules`, `send_safety::deterministic`) come first. The cheap model judge comes second. A person comes last for risky actions. No single layer is trusted alone. [O]
- **Outbound checks are deterministic for leaks:** addresses, phone numbers, IDs or n-grams that appear in the draft but not in the context pack block the send. A model's opinion is not enough (the Muse address incident).
- **Injection signals force approval.** Inbound text that asks to add recipients, forward, ignore rules or call tools makes every send in that thread need a person. [Spotlighting][CaMeL]
- **Hand off to a person** after repeated failures, on high-risk actions, or on anything in `never_do`. [O]

## 6. Models and providers (OpenRouter)
- **Start strong, then swap down.** Set a baseline with the best model on the eval, then move each task to the cheapest model that holds the score. [O]
- Defaults:
  - Understanding, drafts, learner: DeepSeek V4 Flash.
  - Hard cases and "best" agents: V4 Pro.
  - Fast judges: Qwen 3.8 Flash.
  - Never route silently through a generic `OPENROUTER_MODEL_ID`.
  - No Anthropic models in production.
- **Every structured call:** strict `json_schema`, `reasoning: {enabled: false}` (Qwen returned null content with reasoning on), `provider: {data_collection: "deny", require_parameters: true}`. Never combine `parallel_tool_calls` with `require_parameters`; that produced a 404 with no provider. [OR structured outputs, provider routing]
- **Caching:** DeepSeek caches stable prefixes automatically (reads at 0.1×). Send `session_id = "{ws}:{task}"` for sticky routing, and don't set `provider.order` on cached paths. Qwen needs explicit `cache_control` on supported models only. Check `usage.prompt_tokens_details.cached_tokens`. [OR caching][DS]
- **Output caps match the job:** tool calls ≤ 300 tokens, classification ≤ 120, drafts ≤ 700.
- **Retries:** transient errors (429, 5xx, timeout) retry with backoff. Deterministic failures (bad JSON, a link in the draft) get one repair turn, not 5 blind retries.

## 7. Evals before autonomy
- **20–50 tasks from real failures** to start; each needs an unambiguous pass/fail and a reference answer. Test both directions (should reply / should not). [A3]
- **Grade outcomes, not paths.** Code graders first (labels, recipients, `[NEEDS]`, quotes, cost); model judges calibrated against human-rated pairs, with an "unknown" option. [A3]
- **pass^k for anything customer-facing** (sends): all k runs must pass. [A3]
- **Regression suites stay near 100%** (offline golden in CI). Capability suites (live understanding, draft quality) start low and climb. Read the transcripts. [A3]
- Every new path ships with a golden scenario and a cost/latency budget in the live scorecard.

## 8. Observe everything
- Every call goes to `ai_calls` with task, prompt_version, cached/reasoning tokens, provider, finish_reason, error class and ids. Price it from `usage.cost`. [OR usage accounting]
- Incidents (leak, injection, hallucinated extraction, schema failure, refusal, send blocked, auto-send stopped) go to `ai_incidents`. Any incident on an agent demotes it.
- Alert on spend spikes, cost per email above 2× target, p95 latency over budget, schema failures above 2%, and a drop in cache-hit share.

## Before you merge an AI change
1. The prompt version was bumped and the block order follows §3.
2. Untrusted text is fenced or datamarked; nothing derived from an email is in the system message.
3. The cost per call is estimated and listed in docs/ai-engineering-review.md §1.
4. A golden scenario was added; the offline suite is green; the live run shows no safety regressions.
5. Ledger rows show the expected task, model and cached tokens.

## Sources
- [A1] https://www.anthropic.com/engineering/building-effective-agents
- [A2] https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents
- [A3] https://www.anthropic.com/engineering/demystifying-evals-for-ai-agents
- [O] https://cdn.openai.com/business-guides-and-resources/a-practical-guide-to-building-agents.pdf
- [G] https://docs.cloud.google.com/architecture/choose-design-pattern-agentic-ai-system
- [OR] https://openrouter.ai/docs/features/prompt-caching · https://openrouter.ai/docs/features/provider-routing · https://openrouter.ai/docs/features/structured-outputs · https://openrouter.ai/docs/guides/features/tool-calling · https://openrouter.ai/docs/use-cases/usage-accounting
- [DS] https://api-docs.deepseek.com/guides/kv_cache
- Spotlighting https://arxiv.org/abs/2403.14720 · CaMeL https://arxiv.org/abs/2503.18813 · Muse (Sentinel, the address incident) https://xenospectrum.com/en/meta-muse-address-approval-scope/
