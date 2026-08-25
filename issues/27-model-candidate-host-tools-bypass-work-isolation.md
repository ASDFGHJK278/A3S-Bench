# Model Candidate host tools bypass Work isolation

## Summary

The model-backed `a3s-code` Candidate intentionally keeps its existing
`WorkspaceServices::local(workspace)` behavior for compatibility. Task Work
resources are applied to its Docker Bash sandbox, but the embedded controller
and host Git, stash, worktree, file, and command services execute in the Bench
host process rather than the Work container.

This issue records that isolation gap separately. It is not fixed by the
EdgeBench resource-parity work and must not be represented as fixed merely
because Bash containers receive the locked CPU and memory limits.

## Impact

- Host-side Git hooks can execute as the Bench user outside the Work cgroup.
- Git worktree operations can create paths outside the transient submission
  workspace when given sibling or absolute locations accepted by the provider.
- Host-side controller and tool work is not accounted against the Task's Work
  CPU or memory limit.
- Host environment, network, and credential exposure depends on the services
  made available by `WorkspaceServices::local`.

The containerized native Codex Candidate is not affected because its controller
runs inside the Work container.

## Reproduction sketch

1. Run a model-backed `a3s-code` Candidate on a writable workspace.
2. Create a repository with an executable `.git/hooks/post-checkout` hook that
   writes a sentinel outside the workspace.
3. Invoke the exposed host Git checkout operation.
4. Observe the hook running as the Bench host user, outside the Docker Bash
   sandbox and its Task Work resource limits.

## Current compatibility decision

Keep the original `a3s-code` host services available. Continue to apply
`TaskLock.resources.work` to every Docker Bash sandbox invocation. Do not reject
task/v2 solely because the model controller runs outside the Work cgroup.

## Desired follow-up

Provide an opt-in or versioned execution mode that moves the complete model
controller and its tool providers into the Work container, or introduces a
capability-scoped backend that preserves required Git/worktree behavior without
executing hooks or arbitrary host commands.

## Acceptance criteria

- The full model Candidate execution boundary is defined explicitly.
- CPU and memory enforcement covers every Candidate-controlled computation in
  strict isolation mode.
- Git hooks and arbitrary host commands cannot execute outside that boundary.
- Worktree destinations cannot escape the locked workspace.
- Existing compatibility mode remains explicit and identity-bound.
