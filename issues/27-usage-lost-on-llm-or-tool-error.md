# Issue #27: AgentResult.usage lost when LLM call or tool round fails mid-execution

## Summary

When an LLM call or tool execution fails during `execute_loop_inner`,
the `ExecutionLoopState` is dropped via `return Err(e)`, discarding all
accumulated `total_usage` (token counts) and `tool_calls_count`. The
caller (`execute_plan`) has no way to recover these metrics, so the
final `AgentResult.usage` is reported as all zeros — even though the
agent performed extensive work before the failure.

This is a **data-loss bug in a3s-code-core 5.3.4**, not in A3S-Bench.
A3S-Bench faithfully records whatever `AgentResult.usage` it receives.

## Evidence

### Affected run: `rust_multicrate_reconstruction`

| Metric | Trajectory (actual) | Result JSON (recorded) |
|--------|-------------------:|----------------------:|
| prompt_tokens | 15,390,244 | **0** |
| completion_tokens | 158,525 | **0** |
| total_tokens | 15,548,769 | **0** |
| tool_calls_count | 126 | **0** |

- Run ID: `local-1787230883402-2827397-0`
- Status: `completed` (not an error — `execute_plan` returns `Ok`)
- Score: 0.1552
- The trajectory contains 92 `llm_response` events with real `usage`
  data and 126 `tool_call` events, proving the agent worked extensively.

### What happened

1. `PlanningMode::Auto` → pre-analysis decided planning was needed →
   `execute_with_planning` → `execute_plan`
2. `execute_plan` called `execute_loop` for the single non-delegated
   plan step → `execute_loop_inner`
3. 92 LLM turns completed normally; each called
   `state.record_usage(&response.usage)`, accumulating 15.5M tokens in
   `state.total_usage`
4. Turn 92 returned a tool call with malformed JSON arguments (parse
   error). The error was fed back to the LLM.
5. Turn 93: `execute_llm_turn` was called (trajectory seq=438,
   `llm_request` event with 258K estimated prompt tokens). **This LLM
   call failed** — no `llm_response` event follows it. 3.8 seconds
   later, `execution_end` appears with all-zero usage.
6. `execute_loop_inner` hit `Err(e) => return Err(e)` (line 154),
   dropping `ExecutionLoopState` and its 15.5M-token `total_usage`.
7. `execute_plan`'s `Err` branch marked the step `Failed` but had no
   usage to accumulate (the error was a plain `anyhow::Error`).
8. `execute_plan` returned `Ok(AgentResult { usage:
   TokenUsage::default(), tool_calls_count: 0, ... })`.
9. `record_execution_end` recorded zeros. A3S-Bench stored zeros.

### Also affects: `new_foundations_consistency` (timeout path)

| Metric | Trajectory (actual) | Result JSON (recorded) |
|--------|-------------------:|----------------------:|
| prompt_tokens | 38,181,681 | **null** |
| completion_tokens | 384,280 | **null** |
| total_tokens | 38,565,961 | **null** |
| tool_calls_count | 270 | **null** |

- Run ID: `local-1787230883402-2827400-0`
- Status: `timed_out` (12h limit)
- Here the `session.send()` future is dropped by `tokio::time::timeout`,
  so `AgentResult` is never returned at all. The trajectory has no
  `execution_end` event. This is a related but separate concern — see
  "Scope" below.

### Comparison: unaffected runs

| Task | LLM calls | All succeeded? | Recorded usage |
|------|----------:|:--------------:|---------------:|
| exchange_core_throughput | 99 | ✅ | 10,623,804 tokens |
| jagua_nesting_optimization | 193 | ✅ | 25,366,122 tokens |
| rust_multicrate_reconstruction | 92+1 failed | ❌ | **0** |
| new_foundations_consistency | 263 (timeout) | N/A | **null** |

When all LLM calls succeed, usage is correctly accumulated and reported.
The bug only manifests when a mid-execution error drops the state.

## Root Cause

### `ExecutionLoopState` has no error-preserving exit

`src/agent/execution_state.rs` provides two exit methods:

```rust
// line 280 — normal completion: preserves usage ✅
pub(super) fn finish(self, text: String) -> AgentResult {
    AgentResult { usage: self.total_usage, tool_calls_count: self.tool_calls_count, ... }
}

// line 294 — cancelled mid-generation: preserves usage ✅
pub(super) fn finish_interrupted(mut self) -> AgentResult {
    AgentResult { usage: self.total_usage, tool_calls_count: self.tool_calls_count, ... }
}
```

There is **no `finish_error`** — no way to exit with an error while
preserving accumulated metrics.

### `execute_loop_inner` drops state on error

`src/agent/loop_runtime.rs`, three `Err` paths:

```rust
// line 152-154: LLM call failure
Err(_) if cancel_token.is_cancelled() => {
    return Ok(state.finish_interrupted());  // ✅ preserves usage
}
Err(e) => return Err(e),                    // ❌ drops state + usage

// line 164-166: force-finalization tool-call violation
anyhow::bail!(error);                       // ❌ drops state + usage

// line 203-205: tool execution failure
if cancel_token.is_cancelled() {
    return Ok(state.finish_interrupted());  // ✅ preserves usage
}
return Err(e);                               // ❌ drops state + usage
```

The cancel path correctly calls `finish_interrupted()`. The non-cancel
error paths do not — they return a bare `Err(e)`, and `state` (with
its `total_usage`) is dropped.

### `execute_plan` cannot recover usage from `Err`

`src/agent/plan_execution.rs`, single-step error handler (line 454):

```rust
Err(e) => {
    if cancel_token.is_cancelled() { /* break */ }
    tracing::error!("Plan step '{}' failed: {}", step.id, e);
    plan.mark_status(&step.id, TaskStatus::Failed);
    // ❌ no usage recovery — total_usage stays at whatever it was before this step
}
```

The parallel-step error handlers (lines 730, 747) have the same gap.

`execute_plan` returns `Ok(AgentResult { usage: total_usage, ... })`
at line 820, but `total_usage` was only incremented on the `Ok` path
(line 406-409). The `Err` path contributes nothing.

### `record_execution_result` records zeros on `Err`

`src/agent/execution_mode.rs`, line 270:

```rust
Err(e) => {
    self.config.rl_trajectory_recorder.record_execution_end(
        session_id.unwrap_or(""), false, None,
        None,   // ← usage: None
        None,   // ← tool_calls_count: None
        Some(&e.to_string()),
    );
}
```

When `execute_plan` swallows the error and returns `Ok` with zero
usage, this branch is not hit. But when the error propagates all the
way up (non-planning mode), the `execution_end` trajectory event also
records `None` for usage, losing the data even in the trajectory
summary.

## Scope

| Scenario | Affected? | Why |
|----------|:---------:|-----|
| All LLM calls succeed | ❌ | `finish()` preserves usage |
| Cancelled mid-generation | ❌ | `finish_interrupted()` preserves usage |
| LLM call fails (non-cancel) | ✅ | `return Err(e)` drops state |
| Tool execution fails (non-cancel) | ✅ | `return Err(e)` drops state |
| Force-finalization violation | ✅ | `bail!` drops state |
| Timeout (future dropped) | ⚠️ | No `AgentResult` at all — separate issue |

The timeout case (`new_foundations_consistency`) is related but
distinct: the `session.send()` future is dropped by the host's
`tokio::time::timeout`, so no `AgentResult` is ever constructed. Fixing
the error-preservation paths does not help here. The timeout case
requires either (a) a checkpoint-based recovery mechanism or (b)
host-side trajectory parsing to reconstruct usage. This issue focuses
on the non-timeout error paths.

## Proposed Fix

### 1. Add `PartialExecutionResult` carrier type

`src/agent/execution_state.rs`:

```rust
/// Accumulated metrics recovered from a failed `ExecutionLoopState`.
/// Attached as `anyhow::Error` context so callers can downcast and
/// recover usage that would otherwise be lost when the state is dropped.
#[derive(Debug, Clone)]
pub(crate) struct PartialExecutionResult {
    pub(crate) usage: TokenUsage,
    pub(crate) tool_calls_count: usize,
    pub(crate) messages: Vec<Message>,
}
```

### 2. Add `finish_error` to `ExecutionLoopState`

`src/agent/execution_state.rs`:

```rust
/// Exit with an error while preserving accumulated token usage and
/// tool-call counts. The metrics are attached as error context so
/// callers (`execute_plan`) can downcast and recover them.
pub(super) fn finish_error(self, error: anyhow::Error) -> anyhow::Error {
    error.context(PartialExecutionResult {
        usage: self.total_usage,
        tool_calls_count: self.tool_calls_count,
        messages: self.messages,
    })
}
```

### 3. Use `finish_error` in all three `Err` paths

`src/agent/loop_runtime.rs`:

```rust
// line 154: LLM call failure
Err(e) => return Err(state.finish_error(e)),

// line 166: force-finalization violation
return Err(state.finish_error(anyhow::anyhow!(error)));

// line 205: tool execution failure
return Err(state.finish_error(e));
```

### 4. Recover usage in `execute_plan` error handlers

`src/agent/plan_execution.rs`, single-step `Err` branch (line 454):

```rust
Err(e) => {
    if cancel_token.is_cancelled() { /* existing break logic */ }
    // Recover accumulated usage from the failed step
    if let Some(partial) = e.downcast_ref::<PartialExecutionResult>() {
        total_usage.prompt_tokens += partial.usage.prompt_tokens;
        total_usage.completion_tokens += partial.usage.completion_tokens;
        total_usage.total_tokens += partial.usage.total_tokens;
        tool_calls_count += partial.tool_calls_count;
        current_history = partial.messages.clone();
    }
    tracing::error!("Plan step '{}' failed: {}", step.id, e);
    plan.mark_status(&step.id, TaskStatus::Failed);
    /* existing StepEnd logic */
}
```

The parallel-step `Err` handlers (lines 730, 747) need the same
`downcast_ref` recovery.

### 5. Recover usage in `record_execution_result` on `Err`

`src/agent/execution_mode.rs`, `Err` branch (line 270):

```rust
Err(e) => {
    let (usage, tool_calls) = e
        .downcast_ref::<PartialExecutionResult>()
        .map(|p| (Some(&p.usage), Some(p.tool_calls_count)))
        .unwrap_or((None, None));
    self.config.rl_trajectory_recorder.record_execution_end(
        session_id.unwrap_or(""), false,
        None,
        usage,        // ← was None
        tool_calls,   // ← was None
        Some(&e.to_string()),
    );
    /* existing fire_on_error */
}
```

### 6. Export `PartialExecutionResult`

`src/agent.rs`:

```rust
mod execution_state;
pub(crate) use execution_state::{ExecutionSeed, PartialExecutionResult};
```

### Data flow after fix

```
Turn 93 LLM call fails
  → execute_loop_inner: state.finish_error(e)
    → Err(e.context(PartialExecutionResult { 15.5M tokens, 126 calls }))
  → execute_plan: Err branch downcasts PartialExecutionResult
    → total_usage += 15.5M tokens, tool_calls_count += 126
    → marks step Failed
  → execute_plan returns Ok(AgentResult { usage: 15.5M tokens, ... })
  → record_execution_end(usage: 15.5M tokens)
  → AgentResult.usage = 15.5M tokens  ✅
  → A3S-Bench model_usage = 15.5M tokens  ✅
```

## Files to change (a3s-code-core 5.3.4)

| File | Change |
|------|--------|
| `src/agent/execution_state.rs` | Add `PartialExecutionResult` struct + `finish_error` method |
| `src/agent.rs` | Export `PartialExecutionResult` |
| `src/agent/loop_runtime.rs` | Replace 3 bare `Err`/`bail!` exits with `state.finish_error(...)` |
| `src/agent/plan_execution.rs` | Add `downcast_ref::<PartialExecutionResult>` recovery in 3 `Err` handlers |
| `src/agent/execution_mode.rs` | Add `downcast_ref` recovery in `record_execution_result` `Err` branch |

## Deployment

a3s-code-core is a crates.io dependency (`=5.3.4`). Apply the fix via
`[patch.crates-io]` in A3S-Bench's `Cargo.toml`:

```toml
[patch.crates-io]
a3s-code-core = { git = "https://github.com/A3S-Lab/a3s-code-core", branch = "fix/usage-loss-on-error" }
```

## Affected runs

| Run ID | Task | Status | Actual tokens | Recorded tokens | Root cause |
|--------|------|--------|-------------:|----------------:|------------|
| local-1787230883402-2827397-0 | rust_multicrate_reconstruction | completed | 15,548,769 | **0** | LLM call failure dropped state |
| local-1787230883402-2827400-0 | new_foundations_consistency | timed_out | 38,565,961 | **null** | Timeout dropped future (separate concern) |
