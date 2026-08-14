# Containerized Codex Candidate

Status: implementation specification; locally validated with a real model and
benchmark smoke on 2026-08-14.

## Objective and security boundary

The Codex Candidate runs directly inside the Task work Docker container. Bench
invokes the official `bin/codex` entrypoint with
`--dangerously-bypass-approvals-and-sandbox`. There is no bwrap launcher,
nested Codex sandbox, or sandbox preflight. Outer Docker is the sole security
boundary for the Candidate.

The direct-container trust model is intentional: model-generated tools can
read and modify anything visible inside the Task container, including the
copied Codex authentication file. The container does not mount the original
host `auth.json`, benchmark state, Judge artifacts, or the Docker socket.

The container needs network access to reach the Codex control plane. The
production command uses Docker `bridge`; because internal sandboxing is
disabled, that same network is visible to tools inside the container. There is
no per-tool network isolation. A separate outer network policy may change the
container's network exposure, but it must still provide the required control
plane access.

## Prepared official package

Host preparation discovers the installed official standalone package and
prepares it once per artifact digest. Preparation only reads, verifies, copies,
and seals files: it does not download, install, or execute the package. A
subsequent run loads the verified cache entry and bind-mounts it read-only; it
does not install or download Codex in the Task container.

The package verifier binds the package identity to its manifest, target triple,
reported `codex-cli` version, approved layout, regular-file metadata, file
SHA-256 values, and an artifact-set SHA-256 digest. It also checks Linux target
compatibility with the Task platform. Staging uses a private directory,
verification before and after copying, synchronization, read-only sealing, and
atomic publication. Cached entries are resealed and re-verified when loaded,
then fully revalidated immediately before mounting. A same-UID mutation after
that final verification point is outside the trust boundary.

The standalone artifact set contains the Codex executable, its matching
code-mode host, ripgrep, and the official resources. An official bundle may
also carry `codex-resources/bwrap` as an authenticated artifact; this runtime
never invokes it.

## Authentication staging

The default source is `${CODEX_HOME}/auth.json`, falling back to
`$HOME/.codex/auth.json`; `A3S_BENCH_CODEX_AUTH_FILE` is the explicit source
override. Before copying, Bench requires a private regular JSON object, rejects
symlinks, hard links, oversized files, and group/other-readable Unix files,
opens without following links, and checks that file identity, size, and mode
remain stable during the copy.

Each run gets a private home under the benchmark state root. The home and its
`.codex` directory are mode `0700`, the copied `auth.json` is mode `0600`, and
only the benchmark-owned prefix, ownership marker, and age qualify a stale home
for cleanup. Stale owned homes older than 24 hours are removed at staging.
The source `auth.json` is never mounted. Only the per-run private `.codex`
subdirectory is bind-mounted read-write at
`/run/a3s-codex/home/.codex`, so Codex can refresh credentials; all refreshed
data is discarded and is never written back to the host source. The host
private-home root and its ownership marker are not mounted or visible in the
container.

The container receives `HOME=/run/a3s-codex/home` and
`CODEX_HOME=/run/a3s-codex/home/.codex`. No API-key environment variable is
passed. Since the copied file is visible in the container and internal
sandboxing is disabled, tools can read it. This design does not promise that a
model cannot copy those bytes into the workspace or a submission; redaction
protects Bench-captured output, not arbitrary files a tool writes.

In the host process, authentication bytes and every non-empty string value
extracted from the auth document are retained only for redaction and are
cleared when released. Captured stdout and stderr are redacted before event-log
persistence and diagnostics are formed; token-shaped credential values in
failure diagnostics are redacted as well. Authentication is not part of
Candidate identity.

## Container execution

The run binds exactly these host paths:

```text
verified package -> /opt/a3s/codex                 (read-only)
Task workspace   -> /workspace                     (read-write)
private .codex   -> /run/a3s-codex/home/.codex       (read-write)
HOME=/run/a3s-codex/home
CODEX_HOME=/run/a3s-codex/home/.codex
PATH=/opt/a3s/codex/bin:/opt/a3s/codex/codex-path:<minimal system path>
```

The `/run/a3s-codex` tree is a container tmpfs, so the `HOME` parent
`/run/a3s-codex/home` lives on that tmpfs. The host private-home root and
ownership marker remain outside the container and are not visible there.

The command selects `/opt/a3s/codex/bin/codex` as the container entrypoint and
uses `exec`, `--cd /workspace`, `--ephemeral`, `--json`,
`--skip-git-repo-check`, `--ignore-user-config`, `--ignore-rules`, disabled
shell-environment inheritance, and the locked model and reasoning effort when
specified. The package code-mode host path is passed explicitly. No host
environment or API-key variable is inherited into the container.

Docker hardening and work-container limits remain part of the contract:

- read-only root filesystem, all capabilities dropped, and
  `no-new-privileges`;
- `--pids-limit 512`, `--memory 8g`, and `--cpus 4`;
- `/tmp:rw,noexec,nosuid,size=1g` and
  `/run/a3s-codex:rw,noexec,nosuid,nodev,size=64m` temporary filesystems;
- a noninteractive stdin, the Task platform when specified, and the workspace
  owner as the container user.

The container deliberately omits Docker `--rm`, leaving `CodexRunGuard` as the
sole container remover.

The Candidate timeout kills the Docker child at its deadline. Stdout is capped
at 4 MiB and stderr at 512 KiB while both streams continue draining; completed
runs fail if either stream was truncated, and an event log cannot exceed the
stdout bound.

Cleanup is ordered exactly: `CodexRunGuard` first attempts bounded
`docker rm -f` and confirms its success, then deletes the private
authentication home. If container removal cannot be confirmed, the guard
retains the marked home for stale recovery instead of deleting it. The same
order is used by the normal run guard and its best-effort drop path, including
failure and timeout paths; container cleanup itself has a five-second bound.

## Candidate identity and compatibility

Codex Candidates use `a3s.bench.candidate-lock.v3`. The semantic identity
contains the Candidate revision and artifact digest, the `codex-cli` product
and version, the verified package target triple and artifact-set digest, and
the optional model and reasoning effort. Candidate lock creation prepares the
package, and locked loading re-verifies the cached package against those
values.

Codex model values are validated before command construction and lock loading.
Reasoning effort is validated against `none`, `minimal`, `low`, `medium`,
`high`, and `xhigh`. A locked run rejects model or reasoning-effort overrides.
The `run`, advanced Candidate-lock command, and both suite Candidate blocks
accept the same reasoning-effort value; when present, it participates in suite
identity and resume matching. The historical native Codex v2 lock is rejected
with an instruction to regenerate a v3 lock.

For `run` and the advanced Candidate-lock command, an absent CLI value may be
supplied by `bench.codex_reasoning_effort` in `.a3s/config.acl`; the resolved
value is bound into the new lock. Existing locks, AgentTool Candidates, and
suite specs do not inherit that ambient default.

## Verification evidence

The local validation on 2026-08-14 recorded the following real model and
benchmark smoke:

- run `local-1786696785069-149459-0`, Task `quick_file_edit`;
- model `gpt-5.6-luna`, reasoning effort `none`;
- Codex `0.147.0`, target `x86_64-unknown-linux-musl`;
- completed with score `1` and `11` persisted JSONL events;
- scans found zero matches for `access_token`, `refresh_token`,
  `OPENAI_API_KEY`, or `CODEX_API_KEY`;
- cleanup left zero private Codex homes and zero Codex containers.

Full local gates recorded `148` passed, `4` Docker integration tests ignored,
and `0` failed. Formatting, Clippy with warnings denied, built-in validation,
and component-package verification also passed.

This evidence preserves the direct-container trust boundary: tools can read the
copied auth inside the container, and Docker `bridge` is shared with those
tools rather than isolated per tool. A same-UID malicious host mutation after
the immediate-before-mount package verification point is outside the local
trust boundary.
