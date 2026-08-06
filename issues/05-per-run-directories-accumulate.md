Per-run workspace and submission directories are never reclaimed, accumulating indefinitely

Labels: bug

## Symptom

`a3s bench run` creates per-run directories under `.a3s/bench/workspaces/` and `.a3s/bench/submissions/` named `{task_id}-{pid}`, but never cleans them up after the run completes. After batch-evaluating dozens of tasks, these directories (containing full workspace and submission trees) accumulate to tens of GB, wasting disk space.

## Root cause

`execute_inner` creates workspace and submission directories but has no reclamation logic. The submission tree is sealed read-only (0o555) by `seal_role_input_tree`, so a plain `remove_dir_all` silently fails — write permissions must be restored first.

## Environment

- a3s-bench v0.1.2
- Batch evaluation scenarios (dozens of tasks run consecutively)
