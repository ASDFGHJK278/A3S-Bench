Score normalization produces NaN/Inf and crashes benchmark

<!-- 此问题将在存储库 A3S-Lab/Bench (https://github.com/A3S-Lab/Bench) 中创建。更改此行不起任何作用。 -->

代理人: 
标签: bug
里程碑: 
项目: 



<!-- 编辑新问题的正文，然后单击编辑器右上角的 ✓"创建问题"按钮。第一行将是问题标题。代理人和标签紧跟在空白行后面。在开始问题正文之前留出空行。 -->

## Symptom

The judge's score normalization function `normalize_raw()` produces non-finite values (NaN/Inf) in multiple scenarios, triggering an `ensure!` assertion:

```
Judge rescale produced a non-finite value
```

The benchmark crashes and the task cannot complete.

Confirmed affected tasks:
- exchange_core_throughput
- openttd_transport_ai

## Root Cause

`normalize_raw()` has several defects:

1. **Missing non-positive-value guards**: `log_max`, `log_min`, `log_anchor`, `log1p_max` and other kinds call `ln()` directly when raw score ≤ 0, producing -inf/NaN. Each kind should have a `raw <= 0` or `raw <= 1.0` pre-check before applying the logarithm.
2. **Degenerate parameters cause panic**: `linear` kind divides by zero when `upper == lower`; `piecewise` calls `bail!` when consecutive anchor values are equal; `log1p_max` calls `bail!` when denominator is zero.
3. **Missing per-segment clipping in piecewise**: Each piecewise segment should be clipped to its own score range (0–20, 20–80, 80–100) individually. The current code only clamps at the end, so floating-point error can cause out-of-range values.
4. **4 missing rescale kinds**: `min_linear`, `min_linear_positive`, `min_inverse_anchor`, `compression_ratio_cropped_guarded` are not implemented. They fall through to an undefined return value.

## Environment

- a3s-bench built-in judge system (`src/legacy_judge.rs`)
- judge_model: openai/glm-5.2
