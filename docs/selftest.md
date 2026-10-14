# Daily self-test

A GitHub Actions workflow (`.github/workflows/daily-selftest.yml`) runs every day at 03:30 UTC and emails
anukulraj2004@gmail.com when anything fails. It also runs on demand (Actions tab, "daily-selftest", Run workflow).

## What it checks

| Job | Checks |
|---|---|
| `rust` | `cargo clippy --all-targets -D warnings` and the full `cargo test` suite |
| `web` | `pnpm install --frozen-lockfile`, `pnpm typecheck`, `pnpm test --run`, `pnpm build` |
| `smoke` | Over SSH on the VPS: the `app` container is running, `GET http://127.0.0.1:18090/health/ready`, then the public `https://mail.anukulraj.tech/health/ready`, then the model probe below |

### Model probe

`selftest` is a second binary in the API image (`apps/api/src/bin/selftest.rs`, logic in
`apps/api/src/ai/selftest.rs`). The smoke job runs it inside the live container with
`docker compose exec -T app selftest`, so it uses the production environment (API key, model ids).

For each configured model it sends one tool-calling request through `OpenRouterClient::chat_tools`, the same
request builder the planner and the assistant use, with a tiny `ping` tool. The model must answer with a
`ping` tool call. A failing request is retried once after 3 seconds.

Models probed (roles sharing a model id are probed once and listed together, for example `planner+assistant`):

- planner and assistant: `OPENROUTER_PLANNER_MODEL_ID`
- draft: `OPENROUTER_DRAFT_MODEL_ID`
- premium draft: `OPENROUTER_PREMIUM_MODEL_ID`
- classifier: `OPENROUTER_CLASSIFIER_MODEL_ID` (skipped when empty)

Output is one line per model, `ok ...` or `FAIL <roles> (<model>): <reason>`. The exit code is 1 if any model
fails, or if `OPENROUTER_API_KEY` or every model id is missing.

The probe costs a few hundred tokens per model per day.

## Failure and summary emails

- Each failing job writes a report (failed step plus the last 80 log lines) and uploads it as an artifact.
- The `notify-failure` job runs when any job fails. It sends one email over `smtp.hostinger.com:465` (implicit
  TLS) from `SMTP_USERNAME` to the owner. Subject: `[Post Room] daily self-test FAILED – <failed jobs>`.
  Body: failed jobs, run URL, commit, and each job's report.
- On Mondays, when everything passed, `weekly-green` sends a short "all green" email.

The mail action is `dawidd6/action-send-mail`, pinned to a commit SHA (v22).

## Secrets

Same repository secrets as `deploy.yml`: `VPS_SSH_KEY`, `VPS_KNOWN_HOSTS`, `VPS_HOST`, `SMTP_USERNAME`,
`SMTP_PASSWORD`. Nothing new to add.

## Running it manually

- Whole workflow: Actions, daily-selftest, Run workflow.
  - `simulate_failure`: the smoke job fails on purpose, to prove the failure email arrives.
  - `send_summary`: send the all-green email even if it is not Monday.
- Only the model probe, on the VPS: `ssh root@<vps> "cd /docker/email-agents && docker compose exec -T app selftest"`.
- Locally: `cd apps/api && OPENROUTER_API_KEY=... cargo run --bin selftest` (model ids come from the usual env vars).

## Notes

- Scheduled workflows only run from the default branch, so nothing happens until this is merged to `main`.
- The `selftest` binary ships in the image, so the smoke job fails with "executable not found" until the first
  deploy after this change has rolled out.
- GitHub disables scheduled workflows after 60 days without repository activity; re-enable from the Actions tab.
