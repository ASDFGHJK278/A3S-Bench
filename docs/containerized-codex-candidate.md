# Containerized Codex Candidate

Status: implementation specification

## Objective

Run the native Codex candidate inside the Task work container instead of on the
host. The host prepares one immutable copy of the already-installed Codex
package and reuses it for runs. A run copies the host's file-based Codex login
into a private, short-lived container home; it never forwards an API key.

The implementation must support an explicit Codex model and reasoning effort.
Both values are part of Candidate identity and therefore cannot be changed when
executing a locked Candidate.

## Security invariants

1. The model can modify the submission workspace, but it cannot read the host
   login copy, the original host login, benchmark state, Judge files, or Docker
   control interfaces.
2. `auth.json` is copied, never bind-mounted from its original location. The
   copy is readable only by the container Codex process and is removed after
   success, failure, timeout, cancellation, and stale-run recovery.
3. No API-key environment variable is added to the container. The child
   environment is constructed from an allowlist instead of inherited from the
   benchmark process.
4. Authentication bytes, token-shaped values, and the private home are absent
   from Candidate locks, result records, JSONL events, diagnostics, submission
   archives, and normal logs.
5. Codex uses `workspace-write` with approvals disabled. Failure to establish
   that sandbox is fatal. The implementation must never silently retry with
   `danger-full-access` or an equivalent unsandboxed mode.
6. The Judge remains isolated from the work container. The Codex container is
   not given the Docker socket, benchmark state root, or Judge `/tmp`.

## Prepared Codex package

Preparation resolves the active standalone Codex installation once and copies
the complete target-specific package into a content-addressed cache. The
minimum Linux package is:

```text
codex-package.json
bin/codex
bin/codex-code-mode-host
codex-path/rg
codex-resources/bwrap
```

Only copying `bin/codex` is invalid: tool execution requires the matching
code-mode host, and `workspace-write` on Linux requires the packaged sandbox
resource. Preparation validates:

- the package manifest version and target triple;
- regular-file and executable-file requirements without following an
  attacker-controlled final symlink;
- the reported `codex-cli` version;
- per-file SHA-256 values and an artifact-set digest;
- compatibility between the package target and the Task work platform.

The cache is staged in a private directory, verified, made read-only, and
published atomically. Concurrent preparation converges on the same digest.
Runs only bind-mount a verified cached package read-only; they never download or
install Codex in the Task container.

For non-Linux hosts, preparation requires an explicitly supplied official Linux
package for the Task platform. It must still match the locked Codex version and
artifact-set digest. Cross-platform emulation is not inferred silently.

## Authentication staging

The default source is `${CODEX_HOME}/auth.json`, falling back to
`$HOME/.codex/auth.json`. Tests and controlled installations may use an explicit
source override.

Before copying, reject a missing file, directory, symlink, oversized file, or a
Unix file readable by group/other. Open and copy defensively, create the
per-run parent with mode `0700`, and create `auth.json` with mode `0600`. Do not
parse, print, hash into public identity, or persist its contents.

The private home is separate from the submission workspace. It is mounted
read-write because Codex may refresh tokens during a long run; refreshed data
is discarded with the run copy and never written back to the host original.
Startup cleans only stale directories carrying the exact benchmark-owned
prefix and ownership marker.

## Container execution

The Codex execution path reuses the existing work-image lifecycle, limits,
timeout handling, and cleanup, but replaces the Candidate entrypoint with the
cached Codex executable.

Required mounts and environment are conceptually:

```text
cached package -> /opt/a3s/codex                 (read-only)
workspace      -> /workspace                     (read-write)
private home   -> /run/a3s-codex/home            (read-write, not in workspace)
HOME=/run/a3s-codex/home
CODEX_HOME=/run/a3s-codex/home/.codex
PATH=/opt/a3s/codex/bin:/opt/a3s/codex/codex-path:<minimal system path>
```

The outer container keeps a read-only root filesystem, dropped capabilities,
`no-new-privileges`, resource limits, bounded writable tmpfs mounts, a fixed
working directory, and a noninteractive stdin. It is removed on timeout.

Codex itself is invoked with:

- `exec`, JSON events, and ephemeral session state;
- approvals set to `never`;
- `--sandbox workspace-write`;
- `--skip-git-repo-check` because benchmark workspaces need not be Git repos;
- the locked model and reasoning effort;
- inherited shell environment disabled;
- user configuration, rules, and plugins ignored for reproducibility.

The outer container needs network access to the Codex control plane. Tool
network access inside the Codex sandbox follows the Task's declared work
network policy and is not inferred from the outer network mode.

### Sandbox preflight

Before credentials are staged, run the packaged Linux sandbox helper inside the
selected Task image with the exact production Docker security options. Confirm
that it can enter the required namespace and enforce a workspace-only view.

Some Docker Desktop and hardened runtimes deny nested unprivileged user
namespaces even when the packaged static `bwrap` is present. In that case the
run fails before authentication is copied, with a diagnostic that names the
missing runtime capability. Disabling the Codex sandbox, adding broad
capabilities such as `SYS_ADMIN`, or using a privileged container is not an
automatic fallback.

## Candidate identity and compatibility

Introduce a new Candidate lock schema whose Codex identity includes:

- Candidate revision and artifact identity;
- Codex product name and version;
- package target triple and artifact-set digest;
- model, including absence when Codex chooses its default;
- reasoning effort, including absence when Codex chooses its default.

`run`, `advanced candidate lock`, and both suite Candidate blocks accept the
same reasoning-effort option. A locked run rejects model or reasoning overrides.
Suite identity and resume matching include both fields.

Old lock schemas remain readable only with their historical semantics. They
must not be silently reinterpreted as containerized Codex locks; users receive
a clear instruction to regenerate an old native-Codex lock when required.

## Implementation slices

1. **Package and secret preparation**: package discovery, cache manifest,
   defensive auth staging, cleanup guards, and unit tests.
2. **Container executor**: sandbox preflight, Docker command construction,
   timeout cleanup, JSONL capture, and secret-redaction tests.
3. **Identity and CLI**: lock schema, reasoning-effort parsing, suite support,
   compatibility errors, and serialization tests.
4. **Integration and documentation**: wire the new executor into the Codex
   Candidate path, remove host execution, update adapter documentation and
   doctor output, then run end-to-end tests.

These write scopes should remain disjoint while delegated work runs; the parent
integrates them and owns final security review.

## Acceptance evidence

The feature is complete only when all of the following are demonstrated:

1. Two runs with the same installed Codex version reuse one prepared package;
   neither run performs a network download or package installation.
2. A container probe reports the exact host-selected Codex version and the lock
   records the verified package digest and target.
3. An authenticated end-to-end run succeeds using only a copied `auth.json`,
   with API-key variables absent from the container environment.
4. `gpt-5.6-luna` with reasoning effort `none` completes a file-edit smoke Task;
   JSON events and the lock prove the selected model and effort.
5. A malicious Task prompt cannot read either the original or staged
   authentication sentinel, benchmark state, or Judge files. Searches over all
   produced logs, JSONL, records, and submissions find no sentinel.
6. Success, model failure, malformed events, timeout, cancellation, and process
   crash recovery leave no running container and no benchmark-owned credential
   copy.
7. A Task that denies tool network still allows the Codex control plane while a
   model-issued network probe is denied; a Task that permits it passes the same
   probe.
8. Alpine and at least one representative EdgeBench Debian work image pass
   package and sandbox preflight, or fail closed with an actionable unsupported
   runtime diagnostic before credentials are copied.
9. Concurrent package preparation and concurrent Codex runs do not corrupt the
   cache or share mutable homes.
10. Format, Clippy with warnings denied, locked unit tests, built-in validation,
    Docker-backed tests, and component packaging all pass.

## Verified prototype facts

The initial local prototype established that the installed standalone Codex
package is target-specific and reusable, its complete package contains the
Codex executable, code-mode host, ripgrep, and a static PIE `bwrap`, and a
private copied `auth.json` is accepted inside Alpine without an API key. It also
established that `gpt-5.6-luna` accepts reasoning effort `none` and that the
complete binary set can edit a bind-mounted workspace when run unsandboxed.

That unsandboxed edit was only a connectivity prototype, not an accepted
production path. On the current Docker Desktop runtime, the packaged `bwrap`
cannot create its nested user namespace under the intended container security
settings. Production implementation therefore remains gated on the sandbox
preflight behavior above; it must not claim support on that runtime until the
workspace-write smoke test passes.
