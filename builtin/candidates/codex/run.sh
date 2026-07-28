#!/bin/sh
set -eu

# a3s-bench Codex CLI adapter — host runtime
workspace="$1"
test -d "$workspace"

codex_bin="${CODEX_BIN:-codex}"

prompt=""
if [ -n "${A3S_TASK_ROOT:-}" ] && [ -f "${A3S_TASK_ROOT}/public/prompt.md" ]; then
    prompt="$(cat "${A3S_TASK_ROOT}/public/prompt.md")"
fi

if [ -z "$prompt" ]; then
    prompt="Complete the benchmark task in the current workspace. Inspect existing files for instructions."
fi

controller_preamble="You are a benchmark candidate. Complete the supplied task in the workspace directory. Inspect existing files before editing, keep changes scoped to the task, and verify the result when practical. Modify only files inside the workspace. Do not create files outside the workspace."

full_prompt="${controller_preamble}

# Benchmark task

${prompt}

# Workspace contract

The current working directory is the editable workspace root. Write deliverables only inside the workspace. Complete the task and verify the result."

if [ -n "${A3S_GAME_SERVER_URL:-}" ]; then
    full_prompt="${full_prompt}

# Game server

An interactive game server is running at: ${A3S_GAME_SERVER_URL}

You can interact with the game server via HTTP:
- GET  ${A3S_GAME_SERVER_URL}/new      — start a new game session
- POST ${A3S_GAME_SERVER_URL}/action   — send a game action
- GET  ${A3S_GAME_SERVER_URL}/status   — check game status and score

Use curl or python to interact with the game server. Maximize your game score."
fi

codex_exit=0
"$codex_bin" exec \
    -C "$workspace" \
    --skip-git-repo-check \
    --ephemeral \
    --dangerously-bypass-approvals-and-sandbox \
    "$full_prompt" \
    < /dev/null || codex_exit=$?

exit $codex_exit
