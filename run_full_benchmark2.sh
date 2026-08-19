#!/usr/bin/env bash
set -uo pipefail

cd "$(dirname "$0")"
PROJECT_DIR="$PWD"

# Leave the candidate unset unless the caller explicitly supplies one. This
# preserves the default behavior of `a3s bench run` for every candidate.
CANDIDATE="${A3S_BENCH_CANDIDATE:-}"
MODEL="${A3S_BENCH_MODEL:-}"
REASONING_EFFORT="${A3S_BENCH_REASONING_EFFORT:-}"
OUTPUT_ROOT="${A3S_BENCH_OUTPUT_DIR:-$PROJECT_DIR/.test-tmp}"
CODEX_PROXY_HELPER_IMAGE="python@sha256:6d43704baacd1bfbe7c295d7f13079d5d8104ed33568873133f8fc69980419df"

# Capacity failures are transient service-side failures. The default is four
# retries after the initial attempt, with capped exponential backoff.
CAPACITY_RETRIES="${A3S_BENCH_CAPACITY_RETRIES:-4}"
CAPACITY_RETRY_BASE_SECONDS="${A3S_BENCH_CAPACITY_RETRY_BASE_SECONDS:-30}"
CAPACITY_RETRY_MAX_SECONDS="${A3S_BENCH_CAPACITY_RETRY_MAX_SECONDS:-240}"
CAPACITY_ERROR_TEXT="Selected model is at capacity"

usage() {
    cat <<'USAGE'
Usage: ./run_full_benchmark.sh [options] [task-selector ...]

Options:
  --agent CANDIDATE          Candidate reference
  --model MODEL              Pass MODEL through to a3s bench run
  --reasoning-effort LEVEL   Pass LEVEL through to a3s bench run
  -h, --help                 Show this help

Task selectors may be task names, one-based catalog numbers, or ranges such as
3-8. With no selectors, every catalog task is accounted for. Ready tasks run;
blocked tasks are reported with their reason and make the batch return nonzero.

Environment:
  A3S_BENCH_CANDIDATE                    Optional candidate reference. When
                                         omitted, preserve the a3s default.
  A3S_BENCH_MODEL                        Optional model passed through --model.
  A3S_BENCH_REASONING_EFFORT             Optional reasoning effort.
  A3S_BENCH_OUTPUT_DIR                   Log directory (default: .test-tmp).
  A3S_BENCH_CAPACITY_RETRIES             Extra Codex capacity retries
                                         (default: 4; 0 disables retries).
  A3S_BENCH_CAPACITY_RETRY_BASE_SECONDS  First retry delay (default: 30).
  A3S_BENCH_CAPACITY_RETRY_MAX_SECONDS   Maximum retry delay (default: 240).
USAGE
}

require_nonnegative_integer() {
    local name="$1"
    local value="$2"
    if [[ ! "$value" =~ ^[0-9]+$ ]]; then
        echo "$name must be a non-negative integer; got: $value" >&2
        exit 2
    fi
}

require_nonnegative_integer "A3S_BENCH_CAPACITY_RETRIES" "$CAPACITY_RETRIES"
require_nonnegative_integer \
    "A3S_BENCH_CAPACITY_RETRY_BASE_SECONDS" \
    "$CAPACITY_RETRY_BASE_SECONDS"
require_nonnegative_integer \
    "A3S_BENCH_CAPACITY_RETRY_MAX_SECONDS" \
    "$CAPACITY_RETRY_MAX_SECONDS"

SELECTORS=()
while (($# > 0)); do
    case "$1" in
        --agent)
            if (($# < 2)) || [[ -z "$2" ]]; then
                echo "--agent requires a non-empty value." >&2
                exit 2
            fi
            CANDIDATE="$2"
            shift 2
            ;;
        --agent=*)
            CANDIDATE="${1#*=}"
            if [[ -z "$CANDIDATE" ]]; then
                echo "--agent requires a non-empty value." >&2
                exit 2
            fi
            shift
            ;;
        --model)
            if (($# < 2)) || [[ -z "$2" ]]; then
                echo "--model requires a non-empty value." >&2
                exit 2
            fi
            MODEL="$2"
            shift 2
            ;;
        --model=*)
            MODEL="${1#*=}"
            if [[ -z "$MODEL" ]]; then
                echo "--model requires a non-empty value." >&2
                exit 2
            fi
            shift
            ;;
        --reasoning-effort)
            if (($# < 2)) || [[ -z "$2" ]]; then
                echo "--reasoning-effort requires a non-empty value." >&2
                exit 2
            fi
            REASONING_EFFORT="$2"
            shift 2
            ;;
        --reasoning-effort=*)
            REASONING_EFFORT="${1#*=}"
            if [[ -z "$REASONING_EFFORT" ]]; then
                echo "--reasoning-effort requires a non-empty value." >&2
                exit 2
            fi
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        --)
            shift
            SELECTORS+=("$@")
            break
            ;;
        -*)
            echo "Unknown option: $1" >&2
            usage >&2
            exit 2
            ;;
        *)
            SELECTORS+=("$1")
            shift
            ;;
    esac
done

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

retry_delay_seconds() {
    local retry_number="$1"
    local delay="$CAPACITY_RETRY_BASE_SECONDS"
    local step

    # retry_number=1 is the first retry and uses the base delay.
    for ((step = 1; step < retry_number; step++)); do
        if ((delay >= CAPACITY_RETRY_MAX_SECONDS)); then
            break
        fi
        delay=$((delay * 2))
    done

    if ((delay > CAPACITY_RETRY_MAX_SECONDS)); then
        delay="$CAPACITY_RETRY_MAX_SECONDS"
    fi
    printf '%d\n' "$delay"
}

RUN_STAMP="$(date +%Y%m%d-%H%M%S)"
RUN_DIR="$OUTPUT_ROOT/benchmark-$RUN_STAMP"
mkdir -p "$RUN_DIR"
SUMMARY_FILE="$RUN_DIR/summary.log"

{
    echo "A3S Bench Report"
    echo "================"
    echo "Start: $(date)"
    echo "Candidate: ${CANDIDATE:-<a3s default>}"
    echo "Model: ${MODEL:-<candidate default>}"
    if [[ -n "$REASONING_EFFORT" ]]; then
        echo "Reasoning effort: $REASONING_EFFORT"
    fi
    if [[ "$CANDIDATE" == "codex" ]]; then
        echo "Codex capacity retries: $CAPACITY_RETRIES"
        echo "Codex capacity retry delay: ${CAPACITY_RETRY_BASE_SECONDS}s-${CAPACITY_RETRY_MAX_SECONDS}s"
    fi
    echo "Project: $PROJECT_DIR"
    echo
} | tee "$SUMMARY_FILE"

PASSED=0
FAILED=0
BLOCKED=0
SKIPPED=0
MATCHED=0
RETRIED_TASKS=0
CAPACITY_RETRIES_USED=0
TOTAL=${#ALL_TASKS[@]}
HELPER_PREFLIGHT_DONE=0

for offset in "${!ALL_TASKS[@]}"; do
    index=$((offset + 1))
    task="${ALL_TASKS[$offset]}"
    if ! selected "$index" "$task" "${SELECTORS[@]}"; then
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
    : >"$raw_log"
    start_time=$(date +%s)

    command=(a3s bench run "$task")
    if [[ -n "$CANDIDATE" ]]; then
        command+=(--agent "$CANDIDATE")
    fi
    if [[ -n "$MODEL" ]]; then
        command+=(--model "$MODEL")
    fi
    if [[ -n "$REASONING_EFFORT" ]]; then
        command+=(--reasoning-effort "$REASONING_EFFORT")
    fi

    max_attempts=$((CAPACITY_RETRIES + 1))
    attempt=1
    exit_code=0
    final_attempt_log=""

    while :; do
        attempt_log="${raw_log%.log}.attempt-$(printf '%02d' "$attempt").log"
        final_attempt_log="$attempt_log"

        if ((attempt == 1)); then
            echo "[$index/$TOTAL] Running: $task"
        else
            echo "[$index/$TOTAL] Retrying: $task (attempt $attempt/$max_attempts)"
        fi

        {
            printf '\n===== attempt %d/%d: %s =====\n' "$attempt" "$max_attempts" "$(date)"
            printf 'Command:'
            printf ' %q' "${command[@]}"
            printf '\n'
        } >>"$raw_log"

        "${command[@]}" 2>&1 | tee "$attempt_log"
        exit_code=${PIPESTATUS[0]}

        cat "$attempt_log" >>"$raw_log"
        printf '===== attempt %d exit=%d =====\n' "$attempt" "$exit_code" >>"$raw_log"

        if ((exit_code == 0)); then
            break
        fi

        # Do not infer the failure type from exit=1 or exit=2. Retry only when
        # this exact attempt log contains the service-side capacity message.
        if [[ "$CANDIDATE" != "codex" ]] ||
            ! grep -Fq "$CAPACITY_ERROR_TEXT" "$attempt_log"; then
            break
        fi

        if ((attempt >= max_attempts)); then
            echo "Codex capacity retries exhausted for $task after $attempt attempts." \
                | tee -a "$raw_log"
            break
        fi

        retry_number="$attempt"
        delay="$(retry_delay_seconds "$retry_number")"
        CAPACITY_RETRIES_USED=$((CAPACITY_RETRIES_USED + 1))
        echo "Codex model is at capacity for $task; retrying in ${delay}s (next attempt: $((attempt + 1))/$max_attempts)." \
            | tee -a "$raw_log"
        if ((delay > 0)); then
            sleep "$delay"
        fi
        attempt=$((attempt + 1))
    done

    if ((attempt > 1)); then
        RETRIED_TASKS=$((RETRIED_TASKS + 1))
    fi

    duration=$(($(date +%s) - start_time))
    score=$(sed -n 's/.*score=\([^[:space:]]*\).*/\1/p' "$final_attempt_log" | head -n 1)
    score="${score:-N/A}"

    if ((exit_code == 0)); then
        result="PASS"
        PASSED=$((PASSED + 1))
    else
        result="FAIL"
        FAILED=$((FAILED + 1))
    fi
    printf '%-40s | %-7s | %-12s | %dm%02ds | exit=%d | attempts=%d\n' \
        "$task" "$result" "$score" "$((duration / 60))" "$((duration % 60))" \
        "$exit_code" "$attempt" \
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
    echo "Retried tasks: $RETRIED_TASKS"
    echo "Capacity retries used: $CAPACITY_RETRIES_USED"
    echo "End: $(date)"
    echo "Logs: $RUN_DIR"
} | tee -a "$SUMMARY_FILE"

((FAILED == 0 && BLOCKED == 0))
