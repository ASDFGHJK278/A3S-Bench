#!/bin/bash
set -uo pipefail

MODEL="${1:-openai/glm-5.2}"
shift 2>/dev/null || true

cd "$(dirname "$0")"
PROJECT_DIR="$PWD"

# 确保 a3s bench 使用本项目的编译产物，而不是 PATH 上可能过期的版本
export A3S_BENCH_INSTALL_DIR="$PROJECT_DIR/target/debug"

export PATH="$HOME/.cargo/bin:$PATH"

echo ""
echo ">>> Getting task list..."
ALL_TASKS=$(a3s bench list --json | python3 -c "import sys,json; [print(t['id']) for t in json.load(sys.stdin)['data']['tasks']]")
TASK_COUNT=$(echo "$ALL_TASKS" | wc -l)

# 解析参数：序号、范围、任务名混着写
#   ./run_full_benchmark.sh                        -> 全跑
#   ./run_full_benchmark.sh model 37               -> 跑第 37 个
#   ./run_full_benchmark.sh model 37 42            -> 跑第 37 和 42 个
#   ./run_full_benchmark.sh model 37-42            -> 跑第 37~42 个
#   ./run_full_benchmark.sh model 37 42 43-45 task_name -> 混合
MATCH_TASKS=""
if [ $# -eq 0 ]; then
    MATCH_TASKS="$ALL_TASKS"
else
    CONDITIONS=""
    for arg in "$@"; do
        if echo "$arg" | grep -qE '^[0-9]+-[0-9]+$'; then
            start=$(echo "$arg" | cut -d- -f1)
            end=$(echo "$arg" | cut -d- -f2)
            CONDITIONS="$CONDITIONS || (NR>=$start && NR<=$end)"
        elif echo "$arg" | grep -qE '^[0-9]+$'; then
            CONDITIONS="$CONDITIONS || NR==$arg"
        else
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

    RAW_LOG="benchmark-raw-$(date +%Y%m%d-%H%M%S).log"

    # 原样运行，输出同时写到终端和日志文件，不做任何加工
    a3s bench run "$TASK" --agent a3s-code --model "$MODEL" 2>&1 | tee "$RAW_LOG"
    EXIT_CODE=${PIPESTATUS[0]}

    END_TIME=$(date +%s)
    DURATION=$((END_TIME - START_TIME))
    DURATION_STR="$((DURATION / 60))m$((DURATION % 60))s"

    # 判定：exit code 非零就是失败
    if [ "$EXIT_CODE" -eq 0 ]; then
        RESULT="PASS"
        PASSED=$((PASSED + 1))
    else
        RESULT="FAIL"
        FAILED=$((FAILED + 1))
    fi

    SCORE=$(grep -oP 'score=\K[0-9.]+' "$RAW_LOG" | head -1 || echo "N/A")

    # 清理残留容器，忽略错误
    containers=$(docker ps -a --filter "name=a3s-bench" --format "{{.Names}}" 2>/dev/null)
    if [ -n "$containers" ]; then
        docker rm -f $containers 2>/dev/null || true
    fi

    printf "%-40s | %-4s | %-6s | %s | exit=%d\n" "$TASK" "$RESULT" "$SCORE" "$DURATION_STR" "$EXIT_CODE" >> "$SUMMARY_FILE"
    printf "  => %-4s  score=%-6s  time=%s  exit=%d\n" "$RESULT" "$SCORE" "$DURATION_STR" "$EXIT_CODE"
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
