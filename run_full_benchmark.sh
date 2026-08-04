#!/bin/bash
set -uo pipefail

MODEL="${1:-openai/glm-5.2}"
shift 2>/dev/null || true

cd "$(dirname "$0")"
PROJECT_DIR="$PWD"

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

    # 用 --json 输出，可靠解析；exit code 作为第一道失败信号
    a3s bench run "$TASK" --agent a3s-code --model "$MODEL" --json > "$RAW_LOG" 2>&1
    EXIT_CODE=$?

    END_TIME=$(date +%s)
    DURATION=$((END_TIME - START_TIME))
    DURATION_STR="$((DURATION / 60))m$((DURATION % 60))s"

    # 用 Python 解析 JSON，提取 score / status / error，避免 grep 脆弱匹配
    PARSED=$(python3 - "$RAW_LOG" "$EXIT_CODE" <<'PY'
import json, sys
raw_path, exit_code = sys.argv[1], int(sys.argv[2])
try:
    with open(raw_path) as f:
        obj = json.load(f)
except Exception as e:
    print(f"CRASH|N/A|exit_code={exit_code} json_parse_error={e}")
    sys.exit(0)

ok = obj.get("ok", False)
data = obj.get("data", {})
err = obj.get("error", {})
status = data.get("status", "failed")
score = data.get("score", "N/A")
task_id = data.get("task_id", "?")
cand_status = data.get("candidate_execution", {}).get("status", "unknown")

if not ok or exit_code != 0:
    msg = err.get("message", f"exit_code={exit_code}")
    print(f"FAIL|{score}|{msg}")
elif status == "completed":
    print(f"PASS|{score}|cand={cand_status}")
else:
    print(f"FAIL|{score}|status={status}")
PY
)
    # 显示原始日志到终端
    cat "$RAW_LOG"

    RESULT=$(echo "$PARSED" | cut -d'|' -f1)
    SCORE=$(echo "$PARSED" | cut -d'|' -f2)
    DETAIL=$(echo "$PARSED" | cut -d'|' -f3)

    if [ "$RESULT" = "PASS" ]; then
        PASSED=$((PASSED + 1))
    else
        FAILED=$((FAILED + 1))
        echo "  => ERROR: $DETAIL"
    fi

    # 清理残留容器，忽略错误
    containers=$(docker ps -a --filter "name=a3s-bench" --format "{{.Names}}" 2>/dev/null)
    if [ -n "$containers" ]; then
        docker rm -f $containers 2>/dev/null || true
    fi

    printf "%-40s | %-6s | %-6s | %s\n" "$TASK" "$RESULT" "$SCORE" "$DURATION_STR" >> "$SUMMARY_FILE"
    printf "  => %-6s  score=%-6s  time=%s  %s\n" "$RESULT" "$SCORE" "$DURATION_STR" "$DETAIL"
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
