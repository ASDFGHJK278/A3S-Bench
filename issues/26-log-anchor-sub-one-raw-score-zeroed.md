# Issue #26: `log_anchor` rescale zeroes sub-1.0 raw scores — consistent with EdgeBench

## Summary

`ffmpeg_swscale_reimplementation` scored 0 despite the judge producing
`TOTAL_SCORE 0.756346` (correctness 30/30, geometric mean speedup 0.7563x).
Investigation confirms this is **not a bug** — the `log_anchor` rescale
formula intentionally returns 0 for any raw score ≤ 1.0, and A3S-Bench's
implementation is **identical** to EdgeBench's.

## Evidence

### Judge output (run local-1787049879617-2294628-0)

```
TOTAL_SCORE 0.756346
Correctness OK (30/30), geometric mean speedup 0.7563x
exit_code: 0, pass_rate: 1.0, solution_verdict: valid
```

### Rescale config

```json
{
  "kind": "log_anchor",
  "anchor_raw": 14.155,
  "anchor_score": 43.0
}
```

### A3S-Bench `log_anchor` (src/legacy_judge.rs:477-483)

```rust
"log_anchor" => {
    let anchor_raw = get("anchor_raw")?;
    if raw <= 1.0 || anchor_raw <= 1.0 {
        0.0
    } else {
        scale(get("anchor_score")? * raw.ln(), anchor_raw.ln())
    }
}
```

### EdgeBench `log_anchor` (score_rescale.py)

```python
if k == "log_anchor":
    if raw <= 1.0 or spec.anchor_raw <= 1.0:
        return 0.0
    return _clip(spec.anchor_score * math.log(raw) / math.log(spec.anchor_raw))
```

**Both return 0.0 when `raw <= 1.0`.** The guard exists because `ln(x) < 0`
for `x < 1`, which would produce negative scores. For `raw = 0.756346`,
both implementations return 0.

## Why the raw score is < 1.0

The ffmpeg task's `TOTAL_SCORE` is a **geometric mean speedup ratio**
(`baseline_time / candidate_time`):
- `> 1.0` → candidate is faster than baseline → positive score
- `= 1.0` → same speed as baseline → 0 score
- `< 1.0` → candidate is slower than baseline → 0 score

A speedup of 0.7563x means the agent's implementation is ~25% slower
than the baseline. The benchmark awards 0 points for sub-baseline
performance — this is the intended design.

## Minor discrepancy (not affecting this case)

A3S-Bench's `scale()` does **not** clip to [0, 100], while EdgeBench's
`_clip()` does. For `raw > anchor_raw` (14.155), A3S-Bench could produce
scores > 100, while EdgeBench caps at 100. This does not affect sub-1.0
cases (both return 0) but should be fixed for consistency.

## Conclusion

| Aspect | A3S-Bench | EdgeBench | Match? |
|--------|-----------|-----------|--------|
| `raw ≤ 1.0 → 0` | Yes | Yes | ✅ |
| Formula `anchor_score * ln(raw) / ln(anchor_raw)` | Yes | Yes | ✅ |
| Clip to [0, 100] | No | Yes | ❌ (minor) |

The score 0 for `ffmpeg_swscale_reimplementation` is **correct and
consistent with EdgeBench**. This is NOT the same class of bug as
issue #23/24 (pytest_v single-submission scoring).

## Proposed fix (minor)

Add clipping to `normalize_raw()` or `scale()` to match EdgeBench:

```rust
fn scale(numerator: f64, denominator: f64) -> f64 {
    if denominator == 0.0 || !numerator.is_finite() || !denominator.is_finite() {
        0.0
    } else {
        (numerator / denominator).clamp(0.0, 100.0)
    }
}
```

## Affected runs

| Run ID | Task | Date | Raw score | Rescaled | Reason |
|--------|------|------|-----------|----------|--------|
| local-1787049879617-2294628-0 | ffmpeg_swscale_reimplementation | 2026-08-18 | 0.756346 | 0 | log_anchor, raw ≤ 1.0 |
| local-1787006957979-2141767-0 | ffmpeg_swscale_reimplementation | 2026-08-18 | 0.756346 | 0 | log_anchor, raw ≤ 1.0 |
