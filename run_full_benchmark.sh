#!/usr/bin/env bash
set -uo pipefail

cd "$(dirname "$0")"
PROJECT_DIR="$PWD"
export A3S_BENCH_INSTALL_DIR="${A3S_BENCH_INSTALL_DIR:-$PROJECT_DIR/target/debug}"

CANDIDATE="${A3S_BENCH_CANDIDATE:-codex}"
MODEL="${A3S_BENCH_MODEL:-}"
OUTPUT_ROOT="${A3S_BENCH_OUTPUT_DIR:-$PROJECT_DIR/.test-tmp}"

usage() {
    cat <<'EOF'
Usage: ./run_full_benchmark.sh [task-selector ...]

Task selectors may be task names, one-based task numbers, or ranges such as
3-8. With no selectors, every listed task runs.

Environment:
  A3S_BENCH_CANDIDATE  Candidate reference (default: codex)
  A3S_BENCH_MODEL      Optional model passed through --model
  A3S_BENCH_OUTPUT_DIR Directory for logs (default: .test-tmp)
  A3S_BENCH_INSTALL_DIR Bench component directory (default: target/debug)
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    usage
    exit 0
fi

mapfile -t ALL_TASKS < <(
    a3s bench list --json |
        python3 -c 'import json, sys; [print(task["id"]) for task in json.load(sys.stdin)["data"]["tasks"]]'
)

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
SKIPPED=0
MATCHED=0
TOTAL=${#ALL_TASKS[@]}

for offset in "${!ALL_TASKS[@]}"; do
    index=$((offset + 1))
    task="${ALL_TASKS[$offset]}"
    if ! selected "$index" "$task" "$@"; then
        SKIPPED=$((SKIPPED + 1))
        continue
    fi
    MATCHED=$((MATCHED + 1))

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
    printf '%-40s | %-4s | %-12s | %dm%02ds | exit=%d\n' \
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
    echo "End: $(date)"
    echo "Logs: $RUN_DIR"
} | tee -a "$SUMMARY_FILE"

((FAILED == 0))
