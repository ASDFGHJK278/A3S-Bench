#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"
RESULTS_DIR=".a3s/bench/results"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
LOG_FILE="benchmark_${TIMESTAMP}.log"
BATCH_DIR="benchmark_results_${TIMESTAMP}"
MODEL="${1:-openai/gpt-4o}"
RUN_IDS_FILE="$BATCH_DIR/run_ids.txt"
SUMMARY_FILE="$BATCH_DIR/summary.csv"

mkdir -p "$BATCH_DIR"

echo "==========================================" | tee -a "$LOG_FILE"
echo "A3S Code 完整评测开始" | tee -a "$LOG_FILE"
echo "模型: $MODEL" | tee -a "$LOG_FILE"
echo "时间: $(date)" | tee -a "$LOG_FILE"
echo "本批结果目录: $BATCH_DIR" | tee -a "$LOG_FILE"
echo "==========================================" | tee -a "$LOG_FILE"

TASKS=$(a3s bench list | awk '/ready/ {print $1}')
TOTAL=$(echo "$TASKS" | wc -l)
PASS=0
FAIL=0
ALL_RUN_IDS=""

echo "共发现 $TOTAL 个任务" | tee -a "$LOG_FILE"
echo "" | tee -a "$LOG_FILE"

# CSV 表头
echo "task_id,score,run_id,status,duration_seconds" > "$SUMMARY_FILE"

for task in $TASKS; do
    echo "------------------------------------------" | tee -a "$LOG_FILE"
    echo "[$((PASS+FAIL+1))/$TOTAL] 运行: $task" | tee -a "$LOG_FILE"
    START_TIME=$(date +%s)

    set +e
    OUTPUT=$(a3s bench run "$task" --agent a3s-code --model "$MODEL" 2>&1)
    EXIT_CODE=$?
    set -e

    END_TIME=$(date +%s)
    DURATION=$((END_TIME - START_TIME))
    RUN_ID=""
    SCORE=""

    if [ $EXIT_CODE -eq 0 ] && echo "$OUTPUT" | grep -q "COMPLETED"; then
        SCORE=$(echo "$OUTPUT" | grep "COMPLETED" | sed -E 's/.*score=([^ ]+).*/\1/')
        RUN_ID=$(echo "$OUTPUT" | grep "run:" | awk '{print $2}')
        echo "✅ 通过 | 分数: $SCORE | 用时: ${DURATION}s | run: $RUN_ID" | tee -a "$LOG_FILE"
        PASS=$((PASS + 1))
    else
        echo "❌ 失败 | 用时: ${DURATION}s" | tee -a "$LOG_FILE"
        echo "$OUTPUT" | tail -5 | sed 's/^/    /' | tee -a "$LOG_FILE"
        FAIL=$((FAIL + 1))
    fi

    # 记录 run_id
    if [ -n "$RUN_ID" ]; then
        ALL_RUN_IDS="${ALL_RUN_IDS}${RUN_ID}\n"
        echo "$task,$SCORE,$RUN_ID,pass,$DURATION" >> "$SUMMARY_FILE"
    else
        echo "$task,,,fail,$DURATION" >> "$SUMMARY_FILE"
    fi
    echo "" | tee -a "$LOG_FILE"
done

# ==========================================
# 汇总 & 打包结果
# ==========================================
echo "==========================================" | tee -a "$LOG_FILE"
echo "评测完成" | tee -a "$LOG_FILE"
echo "总计: $TOTAL | 通过: $PASS | 失败: $FAIL" | tee -a "$LOG_FILE"

# 将本次的 run_id 写入文件
printf "$ALL_RUN_IDS" > "$RUN_IDS_FILE"

# 将本次的结果文件复制到批处理目录
echo "---" | tee -a "$LOG_FILE"
echo "复制结果文件到 $BATCH_DIR" | tee -a "$LOG_FILE"
COPY_COUNT=0
while IFS= read -r run_id; do
    [ -z "$run_id" ] && continue
    src="$RESULTS_DIR/${run_id}.json"
    if [ -f "$src" ]; then
        cp "$src" "$BATCH_DIR/"
        COPY_COUNT=$((COPY_COUNT + 1))
    fi
done < "$RUN_IDS_FILE"
echo "已复制 $COPY_COUNT 个结果文件" | tee -a "$LOG_FILE"

# 同时也复制日志
cp "$LOG_FILE" "$BATCH_DIR/"

echo "---" | tee -a "$LOG_FILE"
echo "日志: $LOG_FILE" | tee -a "$LOG_FILE"
echo "汇总CSV: $SUMMARY_FILE" | tee -a "$LOG_FILE"
echo "本批结果目录: $BATCH_DIR" | tee -a "$LOG_FILE"
echo "==========================================" | tee -a "$LOG_FILE"

# 本批分数汇总
echo ""
echo "=== 本批分数汇总 ==="
column -t -s ',' "$SUMMARY_FILE"
echo ""
echo "查看JSON结果: ls $BATCH_DIR/"
echo "对比用命令:  cat $SUMMARY_FILE"