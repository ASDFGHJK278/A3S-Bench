Judge timeout runner depends on python3, which is absent from EdgeBench judge images

Labels: bug

## Symptom

Judge execution fails with exit code 127 (command not found) on multiple
EdgeBench tasks (e.g. `ad_placement_optimization`). The entire benchmark run
aborts because exit 127 is not a recognised timeout exit code and falls through
to `abnormal_judge_exit` / `bail!`.

## Root cause

Commit `7b2c840` replaced the previous timeout mechanism with an inline
`python3 -c "subprocess.run(...timeout=N)"` snippet to enforce the judge
descriptor timeout. However, EdgeBench judge images (e.g.
`edgebench.judge.ad_placement_optimization`) are minimal containers that do not
ship `python3`. The `python3` binary is missing, so the judge command exits 127
before any evaluation logic runs.

## Fix

Replace the `python3 -c` timeout runner with GNU coreutils `timeout`:

```
timeout --kill-after=10 <N> /bin/bash -lc '<source_command>'
```

`timeout` is part of coreutils and is present in all the judge base images.
`--kill-after=10` escalates to SIGKILL if the process ignores SIGTERM. The exit
code semantics are unchanged: 124 = timeout, 137 = SIGKILL after grace period.

## Verification

- 106 unit tests pass.
- Manual test against 50 real EdgeBench judge images with a fixed submission:
  previously all returned exit 127; after the fix all return exit 0 and produce
  structured results.
- No EdgeBench judge script traps SIGTERM (checked all 51 images), so the
  SIGTERM → SIGKILL escalation path is not exercised in practice.

## Environment

- a3s-bench dev branch (post `7b2c840`)
- EdgeBench judge images without python3 in PATH
