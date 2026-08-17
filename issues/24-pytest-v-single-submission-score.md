# pytest_v single-submission score must ignore selection_hint

## Problem

The legacy `pytest_v` parser used `selection_hint` when `TOTAL_SCORE` was
absent. `pass_rate_first` returned the pytest pass ratio, while `score_first`
and `valid_then_score` returned zero and an unknown hint failed parsing.

That mixes EdgeBench's multi-submission selection policy into A3S's current
single-submission, single-primary-metric parser. Identical Judge output could
therefore receive different A3S scores solely because of selection metadata.

## Expected behavior

- When `TOTAL_SCORE` exists, normalize it with the locked rescale rule.
- When `TOTAL_SCORE` is absent, use the pytest pass ratio.
- `selection_hint` must not affect either result.
- Explicit zero, scientific notation, non-finite handling, shared score
  extraction, and the `[0,1]` metric range must remain unchanged.

## Resolution

The `pytest_v` branch now uses only the presence of `TOTAL_SCORE` to choose
between normalized raw score and pytest pass ratio. Table-driven tests cover
all known selection hints plus an unknown value for both paths. Existing tests
continue to cover explicit zero versus absence, scientific notation,
`TOTAL_SCORE inf`, non-linear rescaling, partial pytest failure, and reuse by
`score_sum`.

This intentionally does not claim full EdgeBench selection compatibility.
A3S still records one primary score and does not run EdgeBench's submission
