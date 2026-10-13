# Research brief: AI email agents (14 Oct 2026)

Source: web research agent; prices from OpenRouter `/api/v1/models`, tool-calling from OpenRouter tau2-bench airline (2026-10-13).

## Decisions taken from it
- **No Anthropic default.** Drafting default moves to a non-Anthropic model chosen by a blind test on our emails.
- **Planner (tool calling):** `deepseek/deepseek-v4-pro` (tau2 76.4%, ~$0.042/task) or `deepseek/deepseek-v4.1-flash` (76.8%, ~$0.033); `z-ai/glm-5.3` fallback; Kimi K2.6 (74.1%, ~2× cost/task) becomes premium/fallback, not default.
- **Classification/routing:** `deepseek/deepseek-v4-flash`, `z-ai/glm-5.3-flash` or `qwen/qwen3.7-flash` (cheaper than qwen3.8-flash).
- **Vision/OCR:** local parse first (PDF/DOCX/XLSX); images/scans → `qwen/qwen3.8-flash` (multimodal) or `qwen/qwen3-vl-30b-a3b-instruct`.
- **Audio:** `/api/v1/audio/transcriptions` with `qwen/qwen3-asr-1.7b` (~$0.027/h) or `openai/whisper-large-v3-turbo` (~$0.012/h); 60 s upstream timeout → chunk audio.
- **Embeddings:** `qwen/qwen3-embedding-8b` ($0.01/M, 32k ctx) as an alternative to bge-m3.

## Architecture guidance
- Workflows for well-defined steps; agents only where steps are unpredictable (Anthropic "Building effective agents"). Planner turns goals into typed workflow definitions; execution stays deterministic; voice/chat commands use a bounded tool loop.
- Tool risk tiers (OpenAI agents guide): read/label auto; draft auto-to-queue; send/forward/submit/new recipient → approval.
- Durable execution on SQLite: jobs with lease/attempts/next_run/unique key, persisted step outputs, approvals as rows that re-enqueue runs.
- Idempotent side effects: send outbox keyed by (run, step, draft version); `queued → sending → sent`; reconcile before retry; approval stores a hash of the approved content (editing voids it).
- Evals: 20–50 real failure cases; code graders for labels/tool args; LLM judge with rubric calibrated to humans; pass^k for sends; nightly CI; online metric = edit distance before send.
- Cost: store `usage.cost` per call keyed by workspace/user/run/step; budgets per workspace.

## Personalization
- Profile: role, responsibilities, VIPs/key domains, handle vs ignore, tone (formality, length, sign-off, language), approval per action type, working hours, never-do list.
- Onboarding: ~5 questions + paste 1–2 typical replies + import last N sent emails.
- Learning: retrieve the user's own similar sent mail as style examples (Shortwave Ghostwriter); turn each edit into a written, user-visible preference retrieved by similarity (CIPHER/PRELUDE). Memory strictly per user.

## Security (prompt injection)
- Lethal trifecta (private data + untrusted content + external comms) is the default for email agents; EchoLeak (CVE-2025-32711) shows classifier-only defences fail.
- Implement: datamarking/spotlighting of all untrusted text; a quarantined extractor returning typed fields so the planner never sees raw bodies; plan-then-execute with recipients fixed to thread participants/allowlist (new recipient/forward/attachment out → approval); sanitized rendering (no remote images/auto links), strip hidden text from attachments; classifiers only as an extra layer; rate limits + audit.

## Market
- Users value time saved, drafts in their voice, needs-reply vs FYI sorting. Complaints: generic drafts, rigid sorting, billing surprises.
- MVP must-haves: triage with reason, drafts in user's voice grounded in history, never-send default with per-action autonomy, one approval inbox, natural-language rules, thread summaries, unanswered/follow-up detection, attachments, ask-your-inbox, predictable cost.
- Hype for an MVP: fully autonomous sending, 1000s of integrations, multi-agent swarms, voice as primary UI, meeting bots.

## Sources
anthropic.com/engineering/building-effective-agents · anthropic.com/engineering/demystifying-evals-for-ai-agents · OpenAI "A practical guide to building agents" · developers.openai.com voice-agents guide · LangGraph interrupts docs · temporal.io durable execution · docs.rs/apalis · arXiv 2503.18813 (CaMeL) · arXiv 2403.14720 (Spotlighting) · arXiv 2404.15269 (PRELUDE/CIPHER) · simonwillison.net prompt-injection design patterns & lethal trifecta · Microsoft MSRC indirect prompt injection defences · HackTheBox EchoLeak · openrouter.ai models, tau2-bench, usage accounting, STT and PDF docs · MDN Web Speech API · Shortwave pricing & Ghostwriter · Superhuman plans · eesel Fyxer reviews · Google Gmail Gemini blog · Microsoft Copilot Outlook FAQ · Lindy, Zapier, n8n HITL, Relay review, Fin pricing.
