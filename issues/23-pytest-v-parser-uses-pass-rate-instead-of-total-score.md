# Issue #23: pytest_v parser uses pass_rate instead of TOTAL_SCORE

## Summary

The `pytest_v` parser in `legacy_judge.rs` directly returned `pass_rate` as
the final score, ignoring the `TOTAL_SCORE` line and the `rescale_hint`.
This is inconsistent with EdgeBench's grading semantics, where:

1. `parse_pytest_v()` extracts test pass/fail details → computes `pass_rate`
2. `extract_score()` independently extracts `TOTAL_SCORE`
3. `rescale_score()` scales the raw `TOTAL_SCORE` to 0–100
4. `best_score_0_100` (the rescaled TOTAL_SCORE) is the primary leaderboard
   metric, independent of the `selection` policy

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

## Fix (commit a687fac + fec3703)

### First fix (a687fac): Extract TOTAL_SCORE and apply rescale

- Extract `TOTAL_SCORE` from output
- Apply `normalize_raw(rescale, total_score)` to get the rescaled score
- For `score_first`: use the rescaled score directly
- For `pass_rate_first`: use pass_rate when < 1.0, otherwise use rescaled score
- Added `selection_hint` and `score_direction` fields to `LegacyJudgeSource`

### Second fix (fec3703): Always use rescaled TOTAL_SCORE when present

The first fix still used `selection_hint` to decide between pass_rate and
rescaled TOTAL_SCORE for `pass_rate_first` tasks.  However, EdgeBench's
grading flow computes `score_0_100 = rescale_score(rescale, TOTAL_SCORE)`
as the primary leaderboard metric, **independent of selection_hint**.
The `selection_hint` only governs multi-submission selection (which
submission is "best"), not the final score value.  For A3S (single
submission), the selection policy is irrelevant.

The second fix simplifies the parser:
- If `TOTAL_SCORE` is found in output → use rescaled TOTAL_SCORE
- If `TOTAL_SCORE` is absent → fall back to pass_rate
- `selection_hint` no longer affects the score

Also changed `extract_total_score` to return `Option<f64>` to distinguish
"TOTAL_SCORE not present" (None) from "TOTAL_SCORE is 0.0" (Some(0.0)).

Refs #23
