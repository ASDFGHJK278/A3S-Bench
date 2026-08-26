# Issue #29: Unconditional Codex Stop Hook burns quota after convergence

## Summary

The EdgeBench-compatible Codex lifecycle introduced in Issue #25 blocks every
Stop event and asks the agent to continue. This has no convergence condition.
After the agent has finished the implementation and can no longer identify a
meaningful improvement, it repeatedly attempts to stop, receives another
continuation prompt, and restates that no further change is warranted.

For Spark runs, these no-op continuation turns can consume quota much faster
than useful implementation work.

## Evidence

Review of recent quota-bearing Spark conversations found hundreds of assistant
messages in individual runs, including observed counts of 570, 569, and 359.
Late-stage messages repeatedly used phrases such as:

- `No further changes`
- `No additional edits`
- `No further safe deterministic edits`
- `Current final candidate remains ...`

The repeated messages did not correspond to new workspace changes. Conversations
that consumed no quota were excluded from this review.

The previous hook always returned:

```json
{"decision":"block","reason":"Do not stop. Continue working on the implementation."}
```

The outer run loop could also resume a completed Codex process, so merely
allowing the Stop event was insufficient unless the resume loop observed the
same decision.

## Root cause

Issue #25 intentionally copied EdgeBench's unconditional Stop Hook to preserve
harness compatibility. That policy treats every attempted completion as
premature, even when the latest assistant message explicitly says that the
implementation is already complete or that no further safe improvement exists.

The lifecycle therefore had a continuation cap but no semantic convergence
condition. A high cap of 100 resumes bounded process count, but it did not bound
the number or cost of continuation turns inside a Codex session.

## Resolution

Replace the unconditional response with a stateful convergence reviewer:

- Review Codex's `last_assistant_message` on every Stop event.
- Recognize explicit English and Chinese claims that no further meaningful
  change, edit, modification, improvement, or optimization is warranted.
- Keep blocking ordinary completion attempts and ask for another concrete,
  validated implementation pass.
- Count only consecutive convergence claims.
- Reset the count immediately when a later response reports substantive work
  instead of another convergence claim.
- Accept the tenth consecutive convergence claim by returning an empty Stop
  response.
- Write an acceptance marker that the outer run loop checks before issuing
  another `resume --last`.

The threshold is deliberately ten rather than a single final summary or three
repetitions. This keeps the original persistent-review behavior while placing a
finite bound on repeated refusal turns.

## Failure behavior

If the reviewer cannot create or update its state, it fails closed and continues
to block completion. Invalid or missing convergence state is treated as zero.
The state lives only for the current isolated work container and is cleared when
the run loop starts.

The detector intentionally uses the POSIX shell tools already required by the
container harness, rather than assuming Python, Node.js, or `jq` exists in the
Task image.

## Acceptance criteria

- A normal completion message remains blocked.
- Explicit convergence claims 1 through 9 remain blocked and report their
  current count.
- The tenth consecutive convergence claim is accepted.
- A substantive response between convergence claims resets the count.
- English and Chinese convergence wording is covered.
- Once accepted, the outer run loop does not resume the session.
- Shell syntax, focused behavior tests, the full locked test suite, formatting,
  and Clippy all pass.

## Verification

The regression tests exercise ordinary blocking, consecutive counting, reset
behavior, bilingual matching, tenth-claim acceptance, and run-loop suppression.
The full suite completed with 222 tests passed and 8 Docker-dependent tests
ignored; Clippy completed with warnings denied.
