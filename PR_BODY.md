# Performance, correctness, and operational improvements for batch evaluation

## Summary

This PR builds on v0.1.2 with a series of performance optimizations, judge
semantics fixes, and operational tooling needed to run large-scale batch
evaluations. All changes are cleanly ahead of `main` with no divergence.

Closes #1. Closes #2. Closes #3. Closes #4. Closes #5. Closes #6. Closes #7.

## Changes

### Performance

- **Workspace OCI seed caching** (#1): `materialize_seed` now caches extracted
  seeds under `.a3s/bench/workspace-seeds/<sha256>`. On cache hits, the seed
  tree is cloned with `cp -a` instead of re-extracting from Docker. For large
  seeds (248k files / 14 GB), subsequent runs drop from minutes to seconds.

- **Skip redundant chmod on warm path**: Cache hits skip the
  `set_tree_owner_only` traversal since `cp -a` preserves permissions.

- **Remove per-file fsync** (#7): `sync_seed_tree` was calling `fsync` on every
  file in the seed tree, causing multi-minute stalls on ext4 (each `fsync`
  forces a journal commit). Removed in favor of relying on the `.complete`
  marker file for cache validity — the cache can be regenerated if lost to a
  crash.

### Correctness

- **Judge timeout scores 0.0 instead of aborting** (#2): `abnormal_judge_exit`
  no longer treats exit code 124 (timeout) as an infrastructure failure. A
  timeout means the candidate's code was too slow for the judge to complete —
  this is a candidate quality issue. The judge now prints a warning and falls
  through to the normal scoring path, which returns 0.0 when no structured
  result is present.

- **Raw score normalization without rescale config** (#3): When a task has no
  rescale spec, `normalize_raw` now clamps to 0–100 and divides by 100 instead
  of clamping to 0–1. Previously, judges returning percentage-scale scores had
  all values above 1.0 truncated to 1.0.

- **camelCase config compatibility** (#6): `resolve_model_route` now falls back
  to `apiKey`/`baseUrl` when `api_key`/`base_url` is not found, matching the
  behavior of the a3s-code-core config loader. Users no longer need separate
  configs for bench and a3s-code.

### Operational

- **Per-run directory reclamation** (#5): A `TransientRunDirs` RAII guard
  automatically removes workspace and submission directories after the result
  is persisted. `remove_tree` restores write permissions on sealed (0o555)
  directories before deletion, so submissions are correctly reclaimed.

- **Candidate conversation logging** (#4): Model candidate sessions now persist
  full conversation history (session snapshot + JSONL trajectory) to
  `.a3s/bench/runs/<run_id>/`, mirroring EdgeBench's `agent_output.txt`.

- **Batch evaluation scripts**: `run_all_tasks.sh` and `run_full_benchmark.sh`
  provide task selection by index/range/name, `--json` output parsing, per-task
  logging, and CSV summary generation.

## Testing

- `cargo test --locked`: 106 passed, 0 failed, 4 ignored
- `cargo fmt --all -- --check`: clean
- `cargo clippy --locked --all-targets -- -D warnings`: clean

## Issue drafts

Problem descriptions for each fix are documented in `issues/01`–`issues/07`.
