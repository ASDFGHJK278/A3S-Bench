# 31 — Sandbox bash execution has no per-command timeout

## Summary

When a model-backed Candidate runs inside a `DockerBashSandbox`, each `bash`
tool call can block **indefinitely** — there is no per-command timeout on the
sandbox path. A hung process inside the sandbox container stalls the entire
Candidate session until the session-level timeout (up to 43 200 s / 12 h)
expires.

## Reproduction

Task: `exchange_core_throughput`, model: `deepseek-v4-flash-0731`, agent:
`a3s-code`.

The LLM agent issued the following bash command (turn 45) to self-test its
code inside the work container:

```bash
cd /workspace && mvn -B -o test-compile > /tmp/compile.log 2>&1; \
CP="target/classes:target/test-classes:$(find /home/agent/.m2/repository -name '*.jar' | tr '\n' ':')"; \
java -cp "$CP" exchange.core2.core.ScratchPerf
```

`mvn test-compile` succeeded (DNS fix verified), but the `java` command
crashed immediately: the `find *.jar` classpath included stale
`slf4j-jdk14-1.5.6` and `slf4j-nop-1.5.3` jars from Maven plugin transitives,
causing `NoSuchMethodError` in `AffinityLock.<clinit>`. All Disruptor worker
threads died; the main thread blocked forever waiting for them.

The `docker run --rm` spawned by `DockerBashSandbox::exec_command` never
returned. The agent was stuck at `tool_call` for 1 h+ until manual
intervention; without it, the session would hang for the full 12-hour
candidate timeout.

## Root cause

Two layers both miss the timeout:

### 1. a3s-code-core 5.3.4 — `src/tools/builtin/bash.rs`

The bash tool has two execution paths:

```rust
// Sandbox path (lines 174-186) — NO timeout
if let Some(ref sandbox) = ctx.sandbox {
    let result = sandbox
        .exec_command(command, "/workspace")
        .await                    // ← bare await, no tokio::time::timeout
        .map_err(...)?;
    return Ok(ToolOutput { ... });
}

// Local path (lines 202-215) — HAS timeout
let requested_timeout_ms = args.get("timeout")
    .and_then(|v| v.as_u64())
    .unwrap_or(DEFAULT_TIMEOUT_MS);   // 120 000 ms = 2 min
let timeout_ms = requested_timeout_ms.max(MIN_TIMEOUT_MS);
let result = runner.exec(CommandRequest { command, timeout_ms, ... }).await;
```

The sandbox path returns early (`return Ok(...)`) before the timeout logic
runs, so the `timeout` parameter in the tool schema is silently ignored
whenever a sandbox is configured.

### 2. A3S-Bench — `src/model_candidate.rs` `DockerBashSandbox::exec_command`

```rust
let output = docker
    .arg("--workdir").arg("/workspace")
    .arg(&self.image)
    .args(sandbox_shell_argv(&command))
    .output()                       // ← tokio::process::Command, no timeout
    .await
    .context("could not start Docker bash sandbox")?;
```

`docker.output().await` is not wrapped in `tokio::time::timeout`, so the
future resolves only when the container exits — which may never happen.

## Impact

- Any Candidate bash command that hangs (deadlock, infinite loop, waiting on
  dead threads) blocks the entire session for up to `candidate_timeout_sec`
  (12 h for long-horizon tasks).
- The LLM agent cannot recover: it never receives a tool result, so it cannot
  retry or try a different approach.
- Wastes GPU/LLM API budget: the session sits idle but still holds the
  model connection.

## Fix directions (not implemented)

1. **model_candidate.rs (A3S-Bench side):** Wrap `docker.output().await` in
   `tokio::time::timeout(Duration::from_secs(N), ...)`. On timeout, kill the
   container (`docker kill`) and return a `SandboxOutput` with a non-zero
   exit code and a timeout message. Suggested default: 120 s (matching
   `DEFAULT_TIMEOUT_MS`), or read the `timeout` field from the tool args if
   the sandbox trait is extended to accept it.

2. **a3s-code-core (upstream):** In `bash.rs`, apply the same
   `timeout_ms` logic to the sandbox path — either pass it into
   `exec_command` (requires trait change) or wrap the
   `sandbox.exec_command(...).await` in `tokio::time::timeout`.

3. **Docker-level:** Add `--stop-timeout` or run the command inside
   `timeout(1)` within the container (e.g. `timeout 120 /bin/bash -c
   '<command>'`).

## Related

- Discovered while re-running DNS-affected tasks after the DNS fix
  (Issues #28/#30). The SLF4J classpath conflict that triggered the hang is
  itself a Candidate-side mistake (LLM used `find *.jar` to build the
  classpath), but the lack of timeout turned a recoverable error into a
  session-wide deadlock.
