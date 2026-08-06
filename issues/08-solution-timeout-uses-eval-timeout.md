Agent solving timeout mapped to judge eval_timeout instead of EdgeBench's 12-hour agent budget

Labels: bug

## Symptom

All 51 imported EdgeBench tasks have `solution_timeout_sec` set to values between 180 and 21600 seconds. These values are EdgeBench's per-task `eval_timeout` (the judge-script budget), not the agent solving budget. As a result, candidates are killed far too early — e.g. 15 tasks only get 600 seconds to solve, when EdgeBench's official leaderboard allows 43200 seconds (12 hours).

## Root cause

`tools/import_edgebench.py` line 108 reads `task["judge"].get("eval_timeout", 600)` and writes it into `solution_timeout_sec` in the generated `task.acl`. EdgeBench has two independent timeouts:

- `eval_timeout` (per-task, 600–21600s): how long the judge script may run to score one submission.
- `defaults.timeout` (experiment-level, 43200s): how long the agent may work on the task.

The import conflated the two. The `judge.source.json` `evaluation.timeout_sec` correctly uses `eval_timeout`, but `solution_timeout_sec` should have used the agent-level 43200s, which is uniform across all tasks per the official experiment YAMLs (experiment.yaml, experiment-codex.yaml, experiment-glm.yaml, experiment-deepseek.yaml).

## Environment

- a3s-bench v0.1.2
- All 51 EdgeBench-imported tasks in `builtin/tasks/`
- `tools/import_edgebench.py` before this fix
