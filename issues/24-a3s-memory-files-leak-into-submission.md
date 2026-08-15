# Issue #24: `.a3s` directory (including agent memory) leaks into submission

## Summary

The `reserved()` function in `src/submission.rs` only excluded
`.a3s/bench/` paths from the candidate's submission, but not other
`.a3s/` subdirectories such as `.a3s/memory/`.  The a3s-code adapter
stores agent memory (notes, context, etc.) in `.a3s/memory/` inside
the workspace.  These files were included in the submission projected
to the judge.

## Impact

The `ffmpeg_swscale_reimplementation` judge includes a verifier that
scans all submission files for references to FFmpeg internals.  The
agent's `.a3s/memory/index.json` contained such references (from the
agent's own research notes), causing a HARD FAIL with score 0 even
though the agent's actual source code may not have referenced FFmpeg.

Any task whose judge inspects submission file contents (not just
compiled output) is affected.

## Root Cause

```rust
fn reserved(path: &str) -> bool {
    path == ".a3s/bench" || path.starts_with(".a3s/bench/")
}
```

Only `.a3s/bench/` was reserved.  The `.a3s/memory/` directory (and
any other `.a3s/` subdirectory) was not excluded.

## Fix

Changed `reserved()` to exclude the entire `.a3s/` directory:

```rust
fn reserved(path: &str) -> bool {
    path == ".a3s" || path.starts_with(".a3s/")
}
```

Added a test `reserved_excludes_all_a3s_subdirectories` that verifies
`.a3s/memory/`, `.a3s/bench/`, and `.a3s/config.json` are all excluded.
