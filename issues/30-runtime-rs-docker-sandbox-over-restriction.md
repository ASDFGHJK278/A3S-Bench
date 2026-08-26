# Issue #30: Docker sandbox over-restriction in `runtime.rs` and `runtime_profile.rs` (continuation of #28)

## Summary

Issue #28 removed `--read-only`, `--cap-drop ALL`, and `--security-opt
no-new-privileges` from `DockerBashSandbox::exec_command` in
`src/model_candidate.rs` (the per-tool bash sandbox).  However, the same
three flags — plus a `--tmpfs /tmp` overlay — remained in three other
locations that launch the **main candidate work container**, the **judge
container**, the **workspace import staging container**, and the **game
judge container**.  These were never removed because #28 only audited
`model_candidate.rs`.

Additionally, `--tmpfs /tmp:rw,exec,nosuid,size=1g` in
`runtime_profile.rs` shadowed the container image's own `/tmp` with a
1 GB memory-backed tmpfs, which EdgeBench does not do.

## What was left unfixed by #28

### `src/runtime.rs` — three container launch sites

**Location 1: workspace import staging container** (line ~203)

Used to copy files into a Docker volume before the candidate run.  Had
`--read-only`, `--cap-drop ALL`, `--cap-add DAC_OVERRIDE`,
`--security-opt no-new-privileges`.

**Location 2: main candidate work container** (line ~380)

This is the container where the agent actually runs — the most critical
one.  Had `--read-only`, `--cap-drop ALL`,
`--security-opt no-new-privileges`.  Combined with `work_docker_args`
which added `--tmpfs /tmp:rw,exec,nosuid,size=1g`.

**Location 3: legacy judge container** (line ~509)

Runs the judge that evaluates the candidate's submission.  Had
`--read-only`, `--cap-drop ALL`, `--security-opt no-new-privileges`.
Combined with `judge_docker_args` + `READ_ONLY_JUDGE_TMPFS` which added
`--tmpfs /tmp:rw,exec,nosuid,size=4g`.

### `src/game_judge.rs` — game judge container (line ~59)

Had `--read-only`, `--cap-drop ALL`, `--security-opt no-new-privileges`.

### `src/runtime_profile.rs` — tmpfs overlay

`work_docker_args` included `--tmpfs /tmp:rw,exec,nosuid,size=1g`.
`READ_ONLY_JUDGE_TMPFS` was `["--tmpfs", "/tmp:rw,exec,nosuid,size=4g"]`.

`--tmpfs /tmp` mounts a new tmpfs filesystem over `/tmp`, completely
shadowing whatever `/tmp` existed in the container image.  The agent's
`/tmp` is thus a 1 GB RAM disk, not the image's normal directory.

EdgeBench does **not** add `--tmpfs` to any container; it uses the
image's own `/tmp` (in the overlay filesystem, bounded by disk space).

## Bug trigger

### How the over-restriction produced 0-score runs

The chain of causation:

1. `--read-only` makes the container rootfs immutable, so standard cache
   directories (`$HOME/.cache/zig`, `$HOME/.local`, `/var/tmp`) are
   unwritable.
2. The Zig compiler (and many other build tools) write to
   `$HOME/.cache/zig` by default.  When that fails, the agent falls back
   to compiling directly in `/workspace`.
3. Compile artifacts (test binaries, `.o` files, `.zig-global-cache/`)
   end up in the workspace root.
4. These artifacts are included in the submission (because
   `submission_exclude` omits `.zig-global-cache` — Issue #25).
5. Anti-cheat scanner flags `precompiled_objects;stray_elf_binaries` →
   score zeroed.

Even when anti-cheat passed (on runs where the agent happened not to
leave artifacts), `--cap-drop ALL` broke legitimate build operations
that require capabilities like `DAC_OVERRIDE` (for `chown`), `SYS_PTRACE`
(for some linkers), or shared memory allocation.

### `--tmpfs /tmp` size limit

The 1 GB tmpfs at `/tmp` can fill up when an agent writes large
intermediate files (e.g., test repositories, build artifacts) to `/tmp`.
A full `/tmp` causes cryptic "No space left on device" errors that the
agent may not diagnose correctly, wasting turns on irrelevant debugging.

### Evidence from actual runs

| Run ID | Date | Score | Tests passed | Anti-cheat | Tool calls | Root cause |
|--------|------|-------|-------------|------------|------------|------------|
| local-1787037552347-2230636-0 | 08-18 | 0 | 0 | **fail** (precompiled_objects;stray_elf_binaries) | 460 | `--read-only` forced artifacts into workspace |
| local-1787629438104-4126745-0 | 08-25 | 0 | 0 | pass | 90 | vLLM degradation (Issue #29) + `--read-only` prevented compilation |
| local-1787638194716-4159073-0 | 08-25 | **0.000653** | **19** | **pass** | 299 | After removing restrictions from `model_candidate.rs` only |
| — (this fix) | 08-26 | pending | pending | pending | — | After removing restrictions from all locations + tmpfs |

The 08-25 run (local-1787638194716) scored 0.000653 with 19 passing tests
**despite** the `runtime.rs` main container still having the restrictions,
because `model_candidate.rs` (the bash sandbox) was already fixed — the
agent's bash commands ran in sub-containers without the restrictions.
However, the main candidate container itself was still over-restricted,
which could cause issues with agent adapters that run directly in the
main container rather than spawning bash sub-containers.

## Who introduced these and when

All three flags were introduced in the **initial commit** `18e0813` by
**Roy Lin \<roylin@a3s.dev\>** on **2026-07-11** ("feat: implement a3s
bench control component").  The original `--tmpfs` had `noexec` and
`size=64m`.

The design intent (per `docs/design.md` §3) was to treat candidate and
judge code as **containment-untrusted** and use Docker as the sole
security boundary.  This assumption is overly conservative for a
benchmark where the agent is controlled AI, not adversarial malware:

- The container is ephemeral (`--rm`) — anything written inside is
  destroyed after the run.
- Network is already isolated (`--network none` or `bridge`).
- Resource limits (`--memory`, `--cpus`, `--pids-limit`) prevent
  resource exhaustion.
- Docker itself is the isolation boundary — `--cap-drop ALL` and
  `--read-only` add no meaningful protection against container escape
  (which is a Docker/kernel vulnerability, not something these flags
  prevent).

### Partial fixes before this issue

| Date | Author | Commit | What was fixed |
|------|--------|--------|----------------|
| 2026-07-28 | qinmengyuan | `d924c15` | Removed `--cap-drop ALL` from **judge** profile in `runtime_profile.rs` (not from `runtime.rs` container launches) |
| 2026-08-20 | Global Test | `03676c3` | Changed `/tmp` from `noexec` to `exec`; removed `--pids-limit` from judge |
| 2026-08-25 | qmy | `4564fb1` | Removed three flags from `model_candidate.rs` only (Issue #28) |

`runtime.rs` Locations 1-3 and `game_judge.rs` were **never touched**
until this fix.

## Fix

### Removed from `src/runtime.rs`

All three locations (staging, candidate, judge):
```
"--read-only",
"--cap-drop",
"ALL",
"--security-opt",
"no-new-privileges",
```
(plus `"--cap-add", "DAC_OVERRIDE"` at Location 1)

### Removed from `src/game_judge.rs`

Same three flags from the game judge container launch.

### Removed from `src/runtime_profile.rs`

- `--tmpfs /tmp:rw,exec,nosuid,size=1g` from `work_docker_args`
- `READ_ONLY_JUDGE_TMPFS` changed from `["--tmpfs", "/tmp:rw,exec,nosuid,size=4g"]` to `&[]`

Containers now use the image's own `/tmp` (overlay filesystem, bounded by
disk space), matching EdgeBench.

### What remains

Resource limits only — same as EdgeBench:
- `--pids-limit 512` (work container)
- `--memory` (from task config)
- `--cpus` (from task config)
- `--network none` or `bridge` (from task config)

### Not changed

`src/codex_candidate.rs` still has `--read-only`, `--cap-drop ALL`,
`--security-opt no-new-privileges`, and `--tmpfs /run/a3s-codex`.  This
is the Codex CLI adapter (a separate product integration), not the
generic benchmark candidate path.  It should be audited separately if
Codex candidates exhibit similar issues.

## Affected runs

All `git_rewrite_in_zig` runs before the 08-25 `model_candidate.rs` fix
were affected.  Runs using other candidates that execute directly in the
`runtime.rs` main container (rather than via `model_candidate.rs` bash
sub-containers) may still have been affected after the #28 fix and
before this fix.

Refs #25 (anti-cheat false positive — upstream cause).
Refs #28 (partial fix — only `model_candidate.rs` was cleaned).
Refs #29 (vLLM degradation — masked the container issue on 08-25).
