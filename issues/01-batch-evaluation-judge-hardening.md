# Batch evaluation could misreport Judge failures and had stale model-test assumptions

Status: fixed by `d385466` (`fix: harden batch judge evaluation`)

## Problem

Several independent defects made batch evaluation either report an infrastructure failure as a Candidate score or make the model-Candidate test fail for reasons unrelated to the Candidate implementation.

### 1. Judge images could lose their evaluator entrypoint

The common Judge Docker profile mounted a fresh writable tmpfs over `/tmp` for every Judge. Some imported Judge images store their evaluator scripts in `/tmp` (for example, `/tmp/eval_*.sh` and `/tmp/eval_*.py`). The mount hid those image-baked files.

Observed effects included:

- an Ad Placement Judge exiting with code 127 because its script was not found;
- an ANN Judge exiting with code 2;
- both runs being reported as score 0, although the Candidate was not necessarily at fault.

### 2. Candidate-quality failures were difficult to diagnose

When a legacy Judge failed to produce a structured result, the result only exposed a generic zero-score path and insufficient output context. A compiler error at the beginning of a long Judge log could be absent from the stored diagnostics, making it difficult to distinguish a bad Candidate submission from a broken Judge invocation.

### 3. The model-Candidate test no longer matched the active protocol

The local fake model server still rejected streaming requests and returned the older non-SSE response shape. The current `a3s-code-core` path uses SSE and expects an indexed tool-call event. In addition, inherited HTTP proxy variables could route localhost test traffic through an external proxy, producing a misleading timeout.

## Impact

The affected evaluation could produce an incorrect score of zero, waste the full task timeout, or fail a local test without a code regression in the model Candidate itself. Because the issues were in shared Judge and test infrastructure, they could affect multiple task families.

## Fix

The fix separates Docker responsibilities:

- `JUDGE_DOCKER_LIMITS` now contains only PID, memory, and CPU limits;
- `READ_ONLY_JUDGE_TMPFS` is applied only to the read-only Judge profiles that need a writable `/tmp`;
- embedded and game Judges explicitly opt into that tmpfs profile.

Legacy Judge diagnostics now retain bounded UTF-8-safe `output_head` and `output_tail` fields, along with the exit code and parser. Public result projection continues to omit private Judge diagnostics.

The model-Candidate test now:

- serves both streaming SSE and non-streaming responses;
- includes the required tool-call index;
- clears proxy variables under a mutex for the duration of the localhost test and restores them afterward.

## Verification

The corrected Ad Placement run exited with code 0 and produced score `0.1721981475`. The corrected ANN run exited with code 0; its score remained 0 as a legitimate below-baseline result rather than a missing-entrypoint failure.

The public patch passed:

```text
116 Rust tests passed; 4 Docker-backed tests ignored by design
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
python3 tools/check_builtins.py
git diff --check
```

## Follow-up

When adding a new Judge image, do not mount over paths that contain image-baked evaluator assets unless the task explicitly opts into the read-only tmpfs profile. Evaluation automation should retain both the first and last bounded portions of Judge output and should audit the Judge exit code before interpreting a zero score as Candidate quality.
