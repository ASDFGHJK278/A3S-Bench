# Issue #25: Anti-cheat false-positive from uncleaned compile artifacts

## Summary

`git_rewrite_in_zig` tasks are consistently scored 0 by the anti-cheat
check, not because the agent cheated, but because it left development
artifacts (test binaries, `.o` files, `.zig-global-cache`) in the
submission directory. Neither EdgeBench nor A3S-Bench tells the agent to
clean up, and `submission_exclude` omits `.zig-global-cache`.

## Evidence

### Anti-cheat verdict (from judge output)

```json
"anti_cheat": {
    "result": "fail",
    "violations": "precompiled_objects;stray_elf_binaries;",
    "c_loc": 2,
    "strace_cheat": false,
    "strace_details": ""
}
```

### What the artifacts actually are

7 small Zig test programs (`test_z3`, `test_slice`, `test_if`, `test_z4`,
`test_z6`, `test_coerce`, `check_args`) and their `.o` files, left in the
workspace root by the agent during development. Each is ~2.8 MB, debug
build, with its own `test_*.zig` module — the agent compiled standalone
syntax experiments, deleted the `.zig` sources, but forgot to delete the
binaries.

Additionally, `.zig-global-cache/` (Zig compiler's standard global cache,
containing `libcompiler_rt.a` and `.a.o`) was present because
`submission_exclude` does not list it.

### These are NOT cheating

- `strace_cheat: false` — the binary does not shell out to system git.
- `links_libgit2: false` — does not link against libgit2.
- `c_loc: 2` — almost no C code in the submission.
- The actual submission (`src/main.zig`) is a pure-Zig git implementation.

## Root causes

### 1. Prompt does not warn the agent

Neither EdgeBench nor A3S-Bench tells the agent:
- To clean up compile artifacts before submission.
- That the workspace will be submitted as-is.
- That anti-cheat will scan for precompiled objects and stray ELF binaries.
- Which directories are excluded from submission.

**EdgeBench** (`generate_evolve_prompt`):
> Only the following paths are submitted for evaluation: `.`
> Keep these files in a compilable/runnable state at all times.

This says what *will* be submitted, not what *should not* be left behind.
It even says "experimentation is encouraged".

**A3S-Bench** (`controller.md` + `workspace_contract`):
> all deliverable writes must stay inside `/workspace`

This constrains *where* to write, not *what* to leave behind. The agent
is never told about submission or anti-cheat.

### 2. `submission_exclude` is incomplete

```
exclude = [".git", "zig-cache", "zig-out", ".zig-cache",
           "zig-port/zig-cache", "zig-port/zig-out", "zig-port/.zig-cache"]
```

Missing: `.zig-global-cache` — Zig's standard global cache directory,
auto-created by the compiler. This is the same omission in both EdgeBench
and A3S-Bench task specs.

### 3. Anti-cheat `find` excludes do not match `submission_exclude`

The anti-cheat script in `test.sh` excludes `*/.zig-cache/*`,
`*/zig-cache/*`, `*/zig-out/*` via `find -not -path`. Even if
`submission_exclude` were fixed, the `find` patterns would need updating
too. But since the judge image is an OCI artifact, this can only be fixed
upstream in EdgeBench.

## Impact

Any agent that compiles test programs or triggers `.zig-global-cache`
creation in the workspace will be scored 0, regardless of code quality.
The git test suite actually ran (1006/1007 scripts, 560 with at least one
pass) but the score was zeroed by anti-cheat.

## Proposed fixes

### A3S-Bench side (we can do)

1. Add `.zig-global-cache` to `submission_exclude` in `task.acl`.
2. Add a line to `controller.md` or workspace contract warning the agent
   to clean up temporary compile artifacts and not leave binaries/`.o`
   files outside excluded directories.

### Upstream (EdgeBench side)

3. Fix `submission_exclude` in the EdgeBench task spec to include
   `.zig-global-cache`.
4. Update `test.sh` anti-cheat `find` patterns to also exclude
   `.zig-global-cache`.
5. Add cleanup guidance to the task prompt.

## Affected runs

| Run ID | Task | Date | Score | Reason |
|--------|------|------|-------|--------|
| local-1787000181124-2120322-0 | git_rewrite_in_zig | 2026-08-18 | 0 | precompiled_objects; stray_elf_binaries |
| local-1787037552347-2230636-0 | git_rewrite_in_zig | 2026-08-18 | 0 | precompiled_objects; stray_elf_binaries |

