Candidate conversation history not persisted, preventing post-hoc analysis

Labels: enhancement

## Symptom

After a model candidate finishes execution, its full conversation history (tool calls, model responses, intermediate reasoning) is not saved. Once the evaluation ends, there is no way to review the candidate's problem-solving process, diagnose failures, or compare behavior across different models.

## Root cause

`execute_candidate` does not write session snapshots or trajectories to disk. EdgeBench provides an `agent_output.txt` mechanism for this purpose; a3s-bench lacks an equivalent.

## Environment

- a3s-bench v0.1.2
- Any evaluation using a model candidate
