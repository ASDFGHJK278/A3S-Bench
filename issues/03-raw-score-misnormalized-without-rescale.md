Raw score clamped to 1.0 when no rescale config is present

Labels: bug

## Symptom

Some judges return raw scores in the 0–100 range. When a task has no rescale configuration, `normalize_raw` executes `raw.clamp(0.0, 1.0)`, truncating any score above 1.0 to exactly 1.0. This severely distorts candidate scores.

## Root cause

The `spec = None` branch of `normalize_raw` assumes the raw score is already in the 0–1 range. In practice, some judges return percentage-scale scores. The missing step is normalization from 0–100 to 0–1.

## Environment

- a3s-bench v0.1.2
- Tasks without a rescale spec + judges returning percentage-scale scores
