#!/bin/bash
set -euo pipefail

MODEL="${1:-openai/deepseek-v4-flash}"
shift 2>/dev/null || true

cd "$(dirname "$0")"
PROJECT_DIR="$PWD"

export PATH="$HOME/.cargo/bin:$PATH"

echo ""
echo ">>> Getting task list..."
ALL_TASKS=$(a3s bench list --json | python3 -c "import sys,json; [print(t['id']) for t in json.load(sys.stdin)['data']['tasks']]")
TASK_COUNT=$(echo "$ALL_TASKS" | wc -l)

# 解析参数：序号、范围、任务名混着写
#   ./run.sh                                    -> 全跑
#   ./run.sh model 37                           -> 跑第 37 个
#   ./run.sh model 37 42                        -> 跑第 37 和 42 个
#   ./run.sh model 37-42                        -> 跑第 37~42 个
#   ./run.sh model 37 42 43-45 task_name        -> 混合
MATCH_TASKS=""
if [ $# -eq 0 ]; then
    # 没参数 = 全跑
    MATCH_TASKS="$ALL_TASKS"
else
    # 构建一个 awk 表达式来匹配序号或任务名
    CONDITIONS=""
    for arg in "$@"; do
        if echo "$arg" | grep -qE '^[0-9]+-[0-9]+$'; then
            # 范围：37-42
            start=$(echo "$arg" | cut -d- -f1)
            end=$(echo "$arg" | cut -d- -f2)
            CONDITIONS="$CONDITIONS || (NR>=$start && NR<=$end)"
        elif echo "$arg" | grep -qE '^[0-9]+$'; then
            # 单个序号：37
            CONDITIONS="$CONDITIONS || NR==$arg"
        else
            # 任务名
            CONDITIONS="$CONDITIONS || \$1=="$arg""
        fi
    done
    CONDITIONS=$(echo "$CONDITIONS" | sed 's/^ || //')
    MATCH_TASKS=$(echo "$ALL_TASKS" | awk "{ if ($CONDITIONS) print }")
fi

RUN_COUNT=$(echo "$MATCH_TASKS" | wc -l)
echo "Matched: $RUN_COUNT tasks"

echo "========================================="
echo "A3S Bench Run"
echo "Start time: $(date)"
echo "Model: $MODEL"
echo "Project: $PROJECT_DIR"
echo "========================================="

SUMMARY_FILE="benchmark-$(date +%Y%m%d-%H%M%S).log"
PASSED=0
FAILED=0
SKIPPED=0

{
    echo "A3S Bench Report"
    echo "================"
    echo "Start: $(date)"
    echo "Model: $MODEL"
    echo ""
} > "$SUMMARY_FILE"

# 重新遍历 ALL_TASKS 来获取序号
INDEX=0
for TASK in $ALL_TASKS; do
    INDEX=$((INDEX + 1))
    in_match=$(echo "$MATCH_TASKS" | grep -x "$TASK" || true)
    if [ -z "$in_match" ]; then
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
    a3s bench run "$TASK" --agent a3s-code --model "$MODEL" 2>&1 | tee "$RAW_LOG"
    EXIT_CODE=${PIPESTATUS[0]}
    set -e
    OUTPUT=$(cat "$RAW_LOG")

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
        ERROR=$(echo "$OUTPUT" | grep -oP '(?<=failed: ).*' | head -1 || echo "(no details)")
        echo "  => ERROR: $ERROR"
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