Judge timeout misclassified as infrastructure failure, aborting the entire benchmark run

Labels: bug

## Symptom

When the judge process exits with a timeout (exit code 124), `abnormal_judge_exit` classifies it alongside OOM kills (signal termination) and calls `bail!`, aborting the entire benchmark run. Subsequent tasks cannot proceed.

## Root cause

`abnormal_judge_exit` uses the pattern `None | Some(124 | 137 | 143)`, conflating 124 (timeout) with 137/143 (signal kill). However, a timeout means the candidate's code was too slow for the judge to complete within the full timeout window — this is a candidate quality issue, not an infrastructure fault. The correct behavior is to score 0.0 and continue to the next task.

## Environment

- a3s-bench v0.1.2
- Tasks with short judge timeout windows (triggered when candidate code runs slowly)
