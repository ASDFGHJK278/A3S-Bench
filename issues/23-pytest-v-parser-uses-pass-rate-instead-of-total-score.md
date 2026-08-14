# Issue #23: pytest_v parser uses pass_rate instead of TOTAL_SCORE

## Summary

The `pytest_v` parser in `legacy_judge.rs` directly returned `pass_rate` as
the final score, ignoring the `TOTAL_SCORE` line and the `rescale_hint`.
This is inconsistent with EdgeBench's grading semantics, where:

1. `parse_pytest_v()` extracts test pass/fail details → computes `pass_rate`
2. `extract_score()` independently extracts `TOTAL_SCORE`
3. `rescale_score()` scales the raw `TOTAL_SCORE` to 0–100
4. The `selection` policy (`score_first` / `pass_rate_first`) decides which
   metric becomes the final score

## Affected Tasks

| Task | parser | rescale | selection |
|------|--------|---------|-----------|
| `ffmpeg_swscale_reimplementation` | pytest_v | log_anchor | score_first |
| `git_rewrite_in_zig` | pytest_v | linear | score_first |
| `order_addition_permutation_optimization` | pytest_v | linear | pass_rate_first |

## Root Cause

```rust
// Before (bug)
"pytest_v" => pytest_ratio(output),
```

`pytest_ratio` returns `passed / total` — a 0–1 pass rate. EdgeBench uses
`TOTAL_SCORE` (extracted separately) as the score, then applies rescale.
For `score_first` tasks the pass rate is meaningless; for `pass_rate_first`
tasks the pass rate should only be used when below 100%.

## Fix

- Extract `TOTAL_SCORE` from output
- Apply `normalize_raw(rescale, total_score)` to get the rescaled score
- For `score_first`: use the rescaled score directly
- For `pass_rate_first`: use pass_rate when < 1.0, otherwise use rescaled score
- Added `selection_hint` and `score_direction` fields to `LegacyJudgeSource`

Refs #23
