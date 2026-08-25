# Issue #28: Docker work-container sandbox over-restriction vs EdgeBench

## Summary

`DockerBashSandbox::exec_command` in `src/model_candidate.rs` added three
hardening flags that EdgeBench does not use: `--read-only`,
`--cap-drop ALL`, and `--security-opt no-new-privileges`.  These prevent
agents from writing to standard directories such as `$HOME/.cache/zig`
(the Zig compiler's global cache), forcing compilation artifacts into the
workspace root where they leak into the submission and trigger anti-cheat
false positives (Issue #25).

## Evidence

### Zig cache directory unwritable

During `git_rewrite_in_zig` runs the agent attempted to use
`$HOME/.cache/zig` (Zig's standard global cache path) but the container's
root filesystem was mounted read-only (`--read-only`), so writes failed.
The agent fell back to compiling test programs directly in `/workspace`,
leaving stray ELF binaries and `.o` files that the anti-cheat scanner
flagged as `precompiled_objects;stray_elf_binaries`.

### EdgeBench comparison

EdgeBench's container launch for work images does **not** include any of
these three flags.  A3S-Bench added them as defense-in-depth hardening,
but they are unnecessary because:

- The work container is ephemeral (`--rm`) and isolated.
- The workspace is a bind-mount, not the container rootfs.
- `--cap-drop ALL` prevents legitimate operations that some build systems
  need (e.g., creating shared memory segments, setting process priorities).
- `--read-only` makes the container rootfs immutable, blocking standard
  cache directories (`$HOME/.cache`, `$HOME/.local`, `/var/tmp`).

## Root cause

```rust
// src/model_candidate.rs — DockerBashSandbox::exec_command
docker.args([
    "run",
    "--rm",
    "--read-only",           // ← not in EdgeBench
    "--cap-drop",            // ← not in EdgeBench
    "ALL",                   // ← not in EdgeBench
    "--security-opt",        // ← not in EdgeBench
    "no-new-privileges",     // ← not in EdgeBench
]);
```

These flags were introduced when A3S-Bench diverged from EdgeBench's
container configuration.  No corresponding EdgeBench feature or security
requirement motivated them.

## Impact

| Symptom | Cause |
|---------|-------|
| `$HOME/.cache/zig` unwritable | `--read-only` makes rootfs immutable |
| Agent compiles in `/workspace` | Falls back to workspace because cache dirs are blocked |
| Stray binaries in submission | Compile artifacts left in workspace root |
| Anti-cheat false positive (Issue #25) | Artifacts flagged as `precompiled_objects` |
| Some build tools fail silently | `--cap-drop ALL` removes capabilities needed by compilers/linkers |

## Fix

Removed the three flags from `DockerBashSandbox::exec_command` so the
work-container launch matches EdgeBench's configuration:

```rust
docker.args([
    "run",
    "--rm",
]);
```

The remaining security controls (`WORK_DOCKER_LIMITS`: `--pids-limit 512`,
`--memory 8g`, `--cpus 4`, `--tmpfs /tmp:rw,exec,nosuid,size=1g`) are
retained because EdgeBench has equivalent resource limits.

Refs #25 (anti-cheat false positive from compile artifacts — this is the
upstream root cause that forced artifacts into the workspace).
