# EdgeBench-compatible Codex continuation

## Problem

The containerized Codex adapter carried arguments from its old host-native
scaffold and omitted EdgeBench's outcome-affecting continuation lifecycle.
`--cd` duplicated Docker's working directory, `--color never` duplicated JSON
mode, and `--ephemeral`, `--ignore-user-config`, `--ignore-rules`, and the
A3S-specific shell environment policy changed behavior relative to EdgeBench.
A one-shot final verification hook was not equivalent to EdgeBench's repeated
`resume --last` loop.

## Resolution

- Mount the package-embedded hooks and run-loop assets read-only at `/etc/codex`.
- Use EdgeBench's Stop Hook response verbatim: every stop request returns
  `block` with `Do not stop. Continue working on the implementation.`
- Remove `--ephemeral`, `--ignore-user-config`, `--ignore-rules`, `--cd`, the
  color override, and A3S-specific shell environment policy overrides.
- Run the initial `codex exec`, then automatically run
  `codex exec resume --last ... "Continue working."` after every segment lasting
  at least one second, for at most 100 resumes inside the original Candidate
  timeout.
- Retain only A3S observer/identity arguments that do not replace the
  continuation behavior: JSONL output, non-Git workspace support, and the
  lock-bound model and reasoning effort.

This intentionally follows EdgeBench's mutable session and `--last` semantics.