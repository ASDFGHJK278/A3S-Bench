#!/usr/bin/env bash
set -uo pipefail

cd "$(dirname "$0")"
PROJECT_DIR="$PWD"

CANDIDATE="${A3S_BENCH_CANDIDATE:-codex}"
MODEL="${A3S_BENCH_MODEL:-}"
OUTPUT_ROOT="${A3S_BENCH_OUTPUT_DIR:-$PROJECT_DIR/.test-tmp}"
CODEX_PROXY_HELPER_IMAGE="python@sha256:6d43704baacd1bfbe7c295d7f13079d5d8104ed33568873133f8fc69980419df"

usage() {
    cat <<'EOF'
Usage: ./run_full_benchmark.sh [task-selector ...]

Task selectors may be task names, one-based catalog numbers, or ranges such as
3-8. With no selectors, every catalog task is accounted for. Ready tasks run;
blocked tasks are reported with their reason and make the batch return nonzero.

Environment:
  A3S_BENCH_CANDIDATE  Candidate reference (default: codex)
  A3S_BENCH_MODEL      Optional model passed through --model
  A3S_BENCH_OUTPUT_DIR Directory for logs (default: .test-tmp)
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    usage
    exit 0
fi

if ! a3s bench advanced doctor --json >/dev/null; then
    echo "The active top-level 'a3s bench' component did not pass its readiness check." >&2
    echo "Install the package under test before starting the full benchmark." >&2
    exit 1
fi

TASK_ROWS_TEXT="$(
    a3s bench list --all --json |
        python3 -c '
import json
import sys

tasks = json.load(sys.stdin)["data"]["tasks"]
for task in tasks:
    availability = task["availability"]
    values = (task["id"], availability, task["availability_reason"])
    if availability not in {"ready", "blocked"}:
        raise SystemExit(f"invalid task availability: {availability!r}")
    if any("\t" in value or "\n" in value or "\r" in value for value in values):
        raise SystemExit("task listing fields must be single-line and tab-free")
    print("\t".join(values))
'
)" || exit 1

if [[ -z "$TASK_ROWS_TEXT" ]]; then
    echo "No benchmark tasks were returned by 'a3s bench list --all'." >&2
    exit 1
fi

mapfile -t TASK_ROWS <<<"$TASK_ROWS_TEXT"
ALL_TASKS=()
TASK_AVAILABILITY=()
TASK_AVAILABILITY_REASON=()
for row in "${TASK_ROWS[@]}"; do
    IFS=$'\t' read -r task availability reason <<<"$row"
    ALL_TASKS+=("$task")
    TASK_AVAILABILITY+=("$availability")
    TASK_AVAILABILITY_REASON+=("$reason")
done

if ((${#ALL_TASKS[@]} == 0)); then
    echo "No benchmark tasks were returned by 'a3s bench list'." >&2
    exit 1
fi

selected() {
    local index="$1"
    local task="$2"
    shift 2
    if (($# == 0)); then
        return 0
    fi
    local selector start end
    for selector in "$@"; do
        if [[ "$selector" =~ ^([0-9]+)-([0-9]+)$ ]]; then
            start="${BASH_REMATCH[1]}"
            end="${BASH_REMATCH[2]}"
            if ((index >= start && index <= end)); then
                return 0
            fi
        elif [[ "$selector" =~ ^[0-9]+$ ]]; then
            if ((index == selector)); then
                return 0
            fi
        elif [[ "$task" == "$selector" ]]; then
            return 0
        fi
    done
    return 1
}

RUN_STAMP="$(date +%Y%m%d-%H%M%S)"
RUN_DIR="$OUTPUT_ROOT/benchmark-$RUN_STAMP"
mkdir -p "$RUN_DIR"
SUMMARY_FILE="$RUN_DIR/summary.log"

{
    echo "A3S Bench Report"
    echo "================"
    echo "Start: $(date)"
    echo "Candidate: $CANDIDATE"
    echo "Model: ${MODEL:-<candidate default>}"
    echo "Project: $PROJECT_DIR"
    echo
} | tee "$SUMMARY_FILE"

PASSED=0
FAILED=0
BLOCKED=0
SKIPPED=0
MATCHED=0
TOTAL=${#ALL_TASKS[@]}
HELPER_PREFLIGHT_DONE=0

for offset in "${!ALL_TASKS[@]}"; do
    index=$((offset + 1))
    task="${ALL_TASKS[$offset]}"
    if ! selected "$index" "$task" "$@"; then
        SKIPPED=$((SKIPPED + 1))
        continue
    fi
    MATCHED=$((MATCHED + 1))

    availability="${TASK_AVAILABILITY[$offset]}"
    availability_reason="${TASK_AVAILABILITY_REASON[$offset]}"
    if [[ "$availability" == "blocked" ]]; then
        BLOCKED=$((BLOCKED + 1))
        printf '%-40s | %-7s | %s\n' "$task" "BLOCKED" "$availability_reason" \
            | tee -a "$SUMMARY_FILE"
        continue
    fi

    if [[ "$CANDIDATE" == "codex" && $HELPER_PREFLIGHT_DONE -eq 0 ]]; then
        if ! docker image inspect "$CODEX_PROXY_HELPER_IMAGE" >/dev/null 2>&1; then
            echo "The pinned Codex proxy helper image is missing locally:" >&2
            echo "  $CODEX_PROXY_HELPER_IMAGE" >&2
            echo "Preload it before running the Codex benchmark:" >&2
            echo "  docker pull $CODEX_PROXY_HELPER_IMAGE" >&2
            exit 1
        fi
        HELPER_PREFLIGHT_DONE=1
    fi

    safe_task="${task//[^[:alnum:]_.-]/_}"
    raw_log="$RUN_DIR/$(printf '%03d' "$index")-$safe_task.log"
    start_time=$(date +%s)
    command=(a3s bench run "$task" --agent "$CANDIDATE")
    if [[ -n "$MODEL" ]]; then
        command+=(--model "$MODEL")
    fi

    echo "[$index/$TOTAL] Running: $task"
    "${command[@]}" 2>&1 | tee "$raw_log"
    exit_code=${PIPESTATUS[0]}
    duration=$(($(date +%s) - start_time))
    score=$(sed -n 's/.*score=\([^[:space:]]*\).*/\1/p' "$raw_log" | head -n 1)
    score="${score:-N/A}"

    if ((exit_code == 0)); then
        result="PASS"
        PASSED=$((PASSED + 1))
    else
        result="FAIL"
        FAILED=$((FAILED + 1))
    fi
    printf '%-40s | %-7s | %-12s | %dm%02ds | exit=%d\n' \
        "$task" "$result" "$score" "$((duration / 60))" "$((duration % 60))" "$exit_code" \
        | tee -a "$SUMMARY_FILE"
done

if ((MATCHED == 0)); then
    echo "No tasks matched the supplied selectors." | tee -a "$SUMMARY_FILE" >&2
    exit 2
fi

{
    echo
    echo "Matched: $MATCHED"
    echo "Skipped: $SKIPPED"
    echo "Passed: $PASSED"
    echo "Failed: $FAILED"
    echo "Blocked: $BLOCKED"
    echo "End: $(date)"
    echo "Logs: $RUN_DIR"
} | tee -a "$SUMMARY_FILE"

((FAILED == 0 && BLOCKED == 0))
