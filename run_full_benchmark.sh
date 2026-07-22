#!/bin/bash
set -euo pipefail

MODEL="${1:-openai/deepseek-v4-flash}"
START_FROM="${2:-1}"

cd "$(dirname "$0")"
PROJECT_DIR="$PWD"

echo "========================================="
echo "A3S Bench Full Benchmark"
echo "Start time: $(date)"
echo "Model: $MODEL"
echo "Start from task: $START_FROM"
echo "Project: $PROJECT_DIR"
echo "========================================="

if [ ! -f ".a3s/config.acl" ]; then
    echo "ERROR: .a3s/config.acl not found"
    exit 1
fi

export PATH="$HOME/.cargo/bin:$PATH"

echo ""
echo ">>> Getting task list..."
TASKS=$(a3s bench list --json | python3 -c "import sys,json; [print(t['id']) for t in json.load(sys.stdin)['data']['tasks']]")
TASK_COUNT=$(echo "$TASKS" | wc -l)
echo "Total: $TASK_COUNT tasks"
echo ""

SUMMARY_FILE="benchmark-$(date +%Y%m%d-%H%M%S).log"
PASSED=0
FAILED=0
SKIPPED=0

{
    echo "A3S Bench Report"
    echo "================"
    echo "Start: $(date)"
    echo "Model: $MODEL"
    echo "Start from: $START_FROM"
    echo ""
} > "$SUMMARY_FILE"

INDEX=0
for TASK in $TASKS; do
    INDEX=$((INDEX + 1))

    if [ "$INDEX" -lt "$START_FROM" ]; then
        SKIPPED=$((SKIPPED + 1))
        continue
    fi

    echo ""
    echo "========================================="
    echo "[$INDEX/$TASK_COUNT] Running: $TASK"
    echo "========================================="

    START_TIME=$(date +%s)

    set +e
    RAW_LOG="benchmark-raw-$(date +%Y%m%d-%H%M%S).log"
    OUTPUT=$(a3s bench run "$TASK" --agent a3s-code --model "$MODEL" 2>&1 | tee -a "$RAW_LOG")
    EXIT_CODE=$?
    set -e

    END_TIME=$(date +%s)
    DURATION=$((END_TIME - START_TIME))
    DURATION_STR="$((DURATION / 60))m$((DURATION % 60))s"

    SCORE=$(echo "$OUTPUT" | grep -oP 'score=\K[0-9.]+' | head -1 || echo "N/A")

    RESULT="FAIL"
    if echo "$OUTPUT" | grep -q 'COMPLETED'; then
        RESULT="PASS"
    fi

    if [ "$RESULT" = "PASS" ]; then
        PASSED=$((PASSED + 1))
    else
        FAILED=$((FAILED + 1))
    fi

    docker rm -f $(docker ps -a --filter "name=a3s-bench" --format "{{.Names}}" 2>/dev/null) 2>/dev/null || true

    printf "%-40s | %-4s | %-6s | %s\n" "$TASK" "$RESULT" "$SCORE" "$DURATION_STR" >> "$SUMMARY_FILE"
    printf "  => %-4s  score=%-6s  time=%s\n" "$RESULT" "$SCORE" "$DURATION_STR"
done

{
    echo ""
    echo "================"
    echo "Skipped: $SKIPPED"
    echo "Passed: $PASSED / $TASK_COUNT"
    echo "Failed: $FAILED"
    echo "End: $(date)"
} | tee -a "$SUMMARY_FILE"

echo ""
echo "Results saved: $SUMMARY_FILE"