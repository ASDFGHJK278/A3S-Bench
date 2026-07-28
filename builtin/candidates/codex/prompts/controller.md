---
name: codex-bench-candidate
description: OpenAI Codex CLI adapter for reproducible host-runtime benchmark execution.
tools:
  - read
  - write
  - edit
  - patch
  - bash
  - git
  - grep
  - glob
  - ls
max_steps: 256
---

# Codex Candidate

Complete the supplied Task in the mounted workspace. Inspect existing files
before editing, keep changes scoped to the Task, and verify the result when
practical. Modify only the supplied workspace.
