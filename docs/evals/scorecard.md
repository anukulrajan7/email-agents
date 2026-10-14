# Eval scorecard

One entry per run of the golden evals, newest first (`GOLDEN_WRITE_SCORECARD=1 cargo test --test golden_offline`, `golden_understanding`, `golden_live`). "Pending" scenarios are written ahead of a change that has not merged yet and are left out of the pass rate.

## 2026-10-14 | d4134c8 | labelling, keyword baseline

The offline keyword classifier on the golden emails that name a label. The real reader is scored live (`golden_live`).

| label | emails | correct | accuracy |
|---|---:|---:|---:|
| action_required | 3 | 0 | 0% |
| billing | 2 | 1 | 50% |
| bug_report | 2 | 0 | 0% |
| candidate_referral | 4 | 4 | 100% |
| client_requirement | 4 | 1 | 25% |
| complaint | 4 | 0 | 0% |
| follow_up | 1 | 0 | 0% |
| fyi | 3 | 0 | 0% |
| general | 6 | 4 | 67% |
| invoice | 4 | 4 | 100% |
| leave | 7 | 7 | 100% |
| payroll | 6 | 3 | 50% |
| question | 1 | 0 | 0% |
| sales_lead | 5 | 4 | 80% |
| scheduling | 6 | 3 | 50% |
| timesheet | 7 | 7 | 100% |
| all | 65 | 38 | 58% |

## 2026-10-14 | d4134c8 | offline golden eval

Scripted planner, canned drafter, real routing and gates. 95 scenarios.

| agent | scenarios | pass | pass % | safety failures | avg steps | avg cost / case |
|---|---:|---:|---:|---:|---:|---:|
| Aditya | 6 | 6 | 100% | 0 | 3.7 | n/a |
| Ananya | 7 | 7 | 100% | 0 | 3.9 | n/a |
| Arjun | 8 | 8 | 100% | 0 | 3.8 | n/a |
| Ishaan | 7 | 7 | 100% | 0 | 4.0 | n/a |
| Kabir | 6 | 6 | 100% | 0 | 3.7 | n/a |
| Meera | 36 | 36 | 100% | 0 | 3.4 | n/a |
| Nisha | 6 | 6 | 100% | 0 | 3.7 | n/a |
| Rohan | 1 (+6 pending) | 1 | 100% | 0 | 3.0 | n/a |
| Sneha | 6 | 6 | 100% | 0 | 3.8 | n/a |
| Vikram | 6 | 6 | 100% | 0 | 3.7 | n/a |

All: 89/89 pass (100%).
