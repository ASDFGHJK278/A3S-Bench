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
outer network policy follows the locked Task requirement:

- `network_need = "public_internet"` keeps the work container on Docker
  `bridge`. No proxy sidecar or private network is created, so Codex tools have
  the general egress the Task requested.
- `network_need = "none"` attaches the work container to a per-run Docker
  `--internal` control-plane network. A proxy sidecar joins both that internal
  network and `bridge`, and the Candidate's proxy variables name the sidecar.
  This exposes only the restricted Codex control-plane tunnel; it does not
  grant tools general internet access. For an interactive game Task, the work
  container additionally borrows the separate `--internal` network owned by
  the Task's `GameSession`; the proxy sidecar is not attached to that network.

The sidecar is created on the internal network, which has no default gateway,
and is then attached to `bridge`. That later public attachment naturally becomes
its IPv4 default route; Bench does not use Docker's newer `--gw-priority`
option, so this works with Docker releases before 28. At startup, the helper
identifies the default-route interface and binds only to the sole private,
non-loopback IPv4 on the other interface. It fails closed if either is absent or
ambiguous, and never binds to `0.0.0.0` or the `bridge` address. The explicit
`--listen` option accepts only IPv4 loopback for isolated tests, not production.

The sidecar accepts only HTTP `CONNECT` authority-form requests on port 443.
The exact destination allowlist is `chatgpt.com`, `ab.chatgpt.com`,
`api.openai.com`, and `auth.openai.com`, plus hostnames matching
`^sdmntpr[a-z0-9-]+\.oaiusercontent\.com$`. It rejects userinfo, IP literals,
absolute HTTP requests, non-443 ports, non-ASCII/control bytes, trailing-dot
and suffix tricks, malformed headers, and early tunnel payloads. DNS runs in
the sidecar; only resolved addresses for which Python `ipaddress` reports
`is_global` are used, and the socket connects to the already-resolved address
on port 443.

After returning `200 Connection Established`, the proxy reads at most 64 KiB
for the first TLS handshake, including ClientHello split across TLS records.
It requires exactly one canonical ASCII `host_name` SNI equal to the validated
CONNECT authority. Missing, malformed, duplicate, or different SNI closes the
connection before DNS resolution or any upstream socket is opened. The
validated ClientHello bytes are retained and become the first bytes relayed to
the approved upstream.

For a game Task, Bench validates the protected game URL as
`http://<game-container>:8000`, passes it as `GAME_SERVER_URL`, and sets both
`NO_PROXY` and `no_proxy` to exactly `<game-container>`. No suffix, port, proxy
host, or unrelated destination is included. Spawned Codex shells retain these
two no-proxy variables so their HTTP client can reach the game server directly;
the other proxy variables remain excluded from spawned shells. The injected
game completion contract describes `POST /new` with `{}`, `POST /step` with an
`action`, `GET /status`, and optional `POST /close`. The Judge scores the
session's peak score, and a later `POST /new` resets the current score, peak,
and move count.

The proxy bounds request headers to 16 KiB and 100 lines, DNS plus upstream
connection establishment to ten seconds each, inactive tunnels to five
minutes, absolute tunnel lifetime to one hour, each directional relay buffer
to 1 MiB, each tunnel to 256 MiB, resolved answers to 16, and concurrent
connections to 24. DNS runs in a dedicated spawn-mode child that is terminated
and reaped at the deadline. Relay I/O is nonblocking and selector-driven, so a
stalled direction cannot block its worker in a socket write. The helper emits
neither request headers nor authentication values, request bodies, tunnel
bytes, destination-specific errors, or tracebacks.

## Local proxy helper

The proxy source is `runtime_assets/codex_connect_proxy.py`. It is embedded in
the Bench executable at build time, staged into a private Docker volume for the
run, and mounted read-only at `/opt/a3s-proxy/codex_connect_proxy.py` in the
sidecar. Component packaging verifies that the exact required source bytes are
present in the compiled executable, so the helper cannot be omitted from a
component archive accidentally.

The sidecar uses the fixed local helper image
`python@sha256:6d43704baacd1bfbe7c295d7f13079d5d8104ed33568873133f8fc69980419df`
(resolved from `python:3.12-alpine`), starts with an explicit `python3`
entrypoint and `--bind-internal --port 3128`, and is
created with `--pull never`. Operators must preload that image. Bench never
pulls, installs, or updates the helper image during a benchmark run; a missing
local image fails the run before Codex starts.

## Prepared official package

Host preparation discovers the installed official standalone package and
prepares it once per artifact digest. Preparation only reads, verifies, copies,
and seals files: it does not download, install, or execute the package. A
subsequent run loads the verified cache entry, copies it with `docker cp
--archive` through an unstarted staging container into a private named volume,
and mounts the `codex` subdirectory read-only in the Task container. It does not
install or download Codex in the Task container and does not bind-mount the host
cache.

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
The source `auth.json` and per-run host home are never mounted. Bench copies the
per-run home with `docker cp --archive` through the unstarted staging container
into the `home` subdirectory of a private named volume. That subdirectory is
mounted read-write at `/run/a3s-codex/home`, so Codex can refresh credentials;
all refreshed data is discarded and is never written back to the host source.
The host private-home root and its ownership marker are not visible in the
container. The host copy is deleted only after removal of every owned Docker
resource is confirmed.

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

The run has no host bind mounts. It creates uniquely named, owner-labeled Docker
volumes and an unstarted staging container, then uses `docker cp --archive` to
populate them:

```text
package volume /codex -> /opt/a3s/codex             (read-only)
home volume /home     -> /run/a3s-codex/home         (read-write)
workspace volume      -> Task's original absolute path (read-write)
proxy-tools volume    -> /opt/a3s-proxy              (sidecar, read-only)
HOME=/run/a3s-codex/home
CODEX_HOME=/run/a3s-codex/home/.codex
```

A materialized workspace is copied into the volume's `tree` subdirectory and
mounted back at the Task's locked absolute workspace path. The package is
read-only; home and workspace are read-write. `/run/a3s-codex` is a container
tmpfs that supplies the parent mount point. The host package cache, workspace,
private home, benchmark state, and proxy source are not bind-mounted.

The command selects `/opt/a3s/codex/bin/codex` as the container entrypoint and
uses `exec`, `--cd` with the Task's absolute workspace path, `--ephemeral`,
`--json`, `--skip-git-repo-check`, `--ignore-user-config`, `--ignore-rules`, and
the locked model and reasoning effort when specified.
Shell commands use `shell_environment_policy.inherit=all` so the Task image's
`PATH`, `ELAN_HOME`, and other toolchain environment survive. Default exclusions
remain enabled, and all upper/lower-case proxy variables plus `CODEX_HOME` and
`CODEX_CODE_MODE_HOST_PATH` are explicitly excluded from spawned shells. The
container launch does not inject `PATH`; Docker preserves the work image's
`Config.Env`. The package code-mode host is passed to the parent Codex process
by absolute path. No host environment or API-key variable is inherited into the
container.

Docker hardening and work-container limits remain part of the contract:

- read-only root filesystem, all capabilities dropped, and
  `no-new-privileges`;
- `--pids-limit 512`, `--memory 8g`, and `--cpus 4`;
- `/tmp:rw,noexec,nosuid,size=1g` and
  `/run/a3s-codex:rw,noexec,nosuid,nodev,size=64m` temporary filesystems;
- a noninteractive stdin and the Task platform when specified. Bench does not
  pass Docker `--user`; the work image's configured user is preserved. A copied
  materialized workspace is prepared with directory and file modes that remain
  writable from the Task container.

The main and sidecar containers deliberately omit Docker `--rm`, leaving
`CodexRunGuard` as the sole lifecycle owner.

The Candidate timeout kills the Docker child at its deadline. Stdout is capped
at 4 MiB and stderr at 512 KiB while both streams continue draining; completed
runs fail if either stream was truncated, and an event log cannot exceed the
stdout bound.

Cleanup is ordered main container, proxy sidecar, staging container, internal
network, then package/home/workspace/proxy-tools volumes. Every resource name is
unique per run, every mutable resource carries its expected owner label, and
ownership is verified before removal. Each Docker inspect, stop, removal, or
other bounded operation has a 15-second limit; cleanup retries the ordered pass
for up to 600 seconds to cover delayed daemon mutations and transient resource
dependencies. Only after every owned Docker resource is confirmed absent does
`CodexRunGuard` delete the private host authentication home. Otherwise it
retains the marked home for stale recovery. The same rule applies to success,
failure, timeout, and best-effort drop paths.

The `GameSession` network is borrowed, not a Codex-owned resource: Codex neither
labels it as owned nor includes it in normal cleanup or stale-resource sweeps.
Removing the Codex work container releases its membership. `GameSession`
retains responsibility for stopping the protected game server and removing its
internal network after judging, so the two lifecycles cannot delete one
another's resources.

Before staging authentication for a new Codex run, Bench records a common run
id, host boot id, Bench PID, Linux `/proc/<pid>/stat` start ticks, and creation
time on every run-owned container, internal network, and named volume. It then
enumerates resources carrying the run label and groups them by run id. A group
is active only when its boot id still matches the host and the PID's current
start ticks exactly match the recorded value; this prevents both host-reboot
and PID-reuse mistakes. Inactive groups are swept in container, internal
network, then volume order. Owner labels are revalidated against each exact
resource name before removal, and removal is confirmed. Missing metadata,
inconsistent metadata within a group, or an owner mismatch aborts the sweep
without guessing ownership; legacy resources without the run label are not
automatically touched.

If any Docker create or proxy network-connect command times out, the run is
marked as having a pending daemon mutation. Cleanup must then observe all of
the run's resources continuously absent for at least five seconds before it can
succeed; a late reappearance resets that window. The same 600-second cleanup
deadline bounds this confirmation, and a later run's stale sweep is the final
fallback after process termination or host restart.

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
supplied by `bench.codex_reasoning_effort` in `.a3s/bench/config.acl`; the resolved
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
