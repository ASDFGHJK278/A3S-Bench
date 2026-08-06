# Repository Guidelines

A3S Bench is the benchmark control component for A3S, written in Rust. It
snapshots Tasks, Candidates, and Judges into immutable locks, runs Candidates in
an isolated Runtime, and records identity-bound results. See `README.md` and
`docs/design.md` for the full architecture.

## Project Structure & Module Organization

- `src/` — Rust source. `main.rs` is the CLI entry point; `cli.rs` parses
  commands. Domain modules include `bench_run/`, `lock/`, `submission.rs`,
  `runtime.rs`, `os_runtime/`, `legacy_judge.rs`, `game_judge.rs`, and
  `result_record.rs`. Inline unit tests live in `<module>/tests.rs`.
- `builtin/` — Built-in Tasks, Candidates, Judges, `catalog.json`, and license
  provenance. Verified by `tools/check_builtins.py`.
- `examples/` — Reference Candidates and Judges (e.g. `smoke-candidate`).
- `tools/` — Python and shell helpers: `check_builtins.py`,
  `package_component.py`, `smoke_local.sh`, `smoke_imported.sh`.
- `docs/` — Design notes, candidate adapter guide, and Task ACL spec.
- `issues/` — Numbered problem drafts (`NN-kebab-title.md`) tracked alongside
  fixes.
- `.github/workflows/` — `ci.yml` and `release.yml`.

## Build, Test, and Development Commands

```bash
cargo build                       # Debug build of the a3s-bench binary
cargo run -- run quick_file_edit --agent ./examples/smoke-candidate  # Smoke run
cargo test --locked               # Unit + integration tests (fast subset)
cargo test --locked -- --ignored --nocapture --test-threads=1  # Docker-backed tests
cargo fmt --all -- --check        # Verify formatting
cargo clippy --locked --all-targets -- -D warnings  # Lint (warnings fail CI)
python3 tools/check_builtins.py   # Validate builtin catalog integrity
python3 tools/package_component.py # Build and verify the component package
```

Docker is required for the default Runtime and the `--ignored` integration
tests.

## Coding Style & Naming Conventions

- Formatting is enforced by `rustfmt` (stable toolchain). Run `cargo fmt --all`
  before committing; `--check` runs in CI.
- Clippy must pass with `-D warnings`. Address lints rather than silencing them.
- Modules are `snake_case`; types are `UpperCamelCase`. Lock and result types
  follow the `*Lock` / `*Record` / `*Identity` pattern (e.g. `TaskLock`,
  `ResultRecord`).
- Output envelopes use a versioned schema (`a3s.bench.output.v1`); keep JSON
  field names `snake_case`.

## Testing Guidelines

- Tests use Rust's built-in `#[test]` framework. Per-module unit tests live in
  `src/<module>/tests.rs`; larger integration suites are `#[ignore]`-gated
  Docker tests.
- Name tests descriptively (`fn snapshot_lock_is_idempotent()`), describing the
  behavior under test.
- Always run `cargo test --locked` locally. Run the `--ignored` suite only when
  Docker is available.
- New Tasks or Judges must be reflected in `builtin/catalog.json` and pass
  `tools/check_builtins.py`.

## Commit & Pull Request Guidelines

- Use Conventional Commits prefixes seen in history: `feat:`, `fix:`,
  `docs:`, `chore:`, `refactor:`. Keep the subject line imperative and under 72
  characters.
- Reference issues by number in the body or footer (e.g. `Refs #14`). Problem
  drafts go in `issues/NN-kebab-title.md`.
- PRs must pass CI: formatting, clippy, builtin check, locked tests, and the
  component package build. Docker smoke tests run in the `docker-smoke` job.
- Releases are tag-driven (`v*`); the tag must match the version in
  `Cargo.toml`. Do not bump the version without a matching tag.
