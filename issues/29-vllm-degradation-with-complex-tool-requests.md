# Issue #29: vLLM server degradation with complex tool-bearing requests

## Summary

The vLLM inference server (`token.pjlab.org.cn`, model `deepseek-v4-flash`)
enters an infinite repetition loop when processing chat-completion
requests that include a large number of tool definitions (18 tools + full
system prompt).  Simple requests (e.g., "Say hello") complete normally in
<1 s, but the complex requests that A3S-Bench candidates generate cause
the server to stream megabytes of degenerate, repetitive text until the
client-side `llm_api_timeout_ms` (1 800 000 ms = 30 min) fires.

This produces benchmark runs with **zero passing test cases** and
extremely short execution times (90 tool calls in ~15 min vs. 460 tool
calls in ~4 h on a healthy server), which are indistinguishable from
genuine model failures without trajectory inspection.

## Evidence

### Timeline of degradation

| Date | 1st-turn duration | completion_tokens | stop_reason | Status |
|------|------------------:|------------------:|-------------|--------|
| 08-18 | 2.25 s | 102 | tool_calls | ✅ Normal |
| 08-24 | 87 s | 0 | None | ⚠️ Degraded |
| 08-25 13:35 | 1 800 s (timeout) | 0 | None | ❌ Broken |
| 08-25 14:09 | 2.4 s | 141 | tool_calls | ✅ Recovered |

### Network-level evidence

During a hang the TCP connection to the API server remains established and
actively receiving data — the server is streaming a degenerate response:

```
ESTAB  10.6.20.134:55806 → 10.12.111.139:443
  bytes_received: 12 588 706 (12.0 MB)
  lastrcv: 17 ms ago        ← still streaming
```

The streamed content is repetitive garbage:
> "I'll start by inspecting the workspace... Let me start by exploring
> the workspace..." (repeating)

### Simple requests succeed

A curl test with `{"model":"deepseek-v4-flash","messages":[{"role":"user",
"content":"Say hello"}],"max_tokens":50}` returns normally in 0.34 s with
`stop_reason: "stop"`, `completion_tokens: 12`.

The difference is the tool payload: the candidate's request includes 18
tool definitions + the full A3S Code system prompt (~4 000 tokens of
system context), which appears to trigger a vLLM generation path bug.

### parallel_task amplification

When the agent delegates work via `parallel_task`, the sub-agent's LLM
request triggers the same degradation.  Because `parallel_task` allows 1
retry (`MAX_TRANSIENT_PARALLEL_RETRIES`), the effective hang can reach
~60 min (2 × 30-min timeout).  In one observed run, a `parallel_task`
call at turn 41 blocked the main loop for 46 minutes.

## Impact

| Symptom | Cause |
|---------|-------|
| 0 passing test cases | Agent hangs on first turn, never produces code |
| Very short run duration (15 min) | Only ~90 tool calls before all turns time out |
| Inconsistent results across days | Server degradation is intermittent |
| parallel_task 46-min hang | Sub-agent request triggers same vLLM bug + retry |

## Root cause

External: vLLM server-side generation bug triggered by large tool-bearing
requests.  This is **not** an A3S-Bench code defect, but it severely
impacts benchmark reliability and reproducibility.

## Mitigations (A3S-Bench side)

1. **Detect degenerate responses at the client**: If `stop_reason` is
   `None` and `completion_tokens` is 0 after a response, retry the
   request immediately rather than waiting for the full timeout.

2. **Shorter timeout for first-turn requests**: The first LLM turn should
   complete in <30 s on a healthy server.  A shorter initial timeout
   (e.g., 120 s) would fail fast and allow the run to retry or abort
   early, rather than burning 30 min on a degenerate response.

3. **parallel_task timeout**: Add an explicit `timeout_ms` to
   `parallel_task` calls (currently inherits the global
   `llm_api_timeout_ms`), so sub-agent hangs don't block the main loop
   for 60 min.

4. **Trajectory health check**: After a run completes, flag runs where
   >50 % of LLM responses have `stop_reason: None` or
   `completion_tokens: 0` as "server-degraded" rather than "model-failed".

## Affected runs

| Run ID | Date | Task | Tool calls | Pass | Cause |
|--------|------|------|--------:|-----:|-------|
| local-1787629438104-4126745-0 | 08-25 | git_rewrite_in_zig | 90 | 0 | All turns timed out on degenerate vLLM |
| local-1787572227499-3856382-0 | 08-24 | git_rewrite_in_zig | — | 0 | Degraded vLLM, anti-cheat pass but 0 tests passed |
