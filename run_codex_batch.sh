#!/bin/bash
# Full task suite test for Codex adapter
# Runs all 52 tasks sorted by timeout (shortest first)

cd /home/qmy/projects/a3s-bench
BENCH="./target/release/a3s-bench"
RESULTS_DIR="codex_results_$(date +%Y%m%d_%H%M%S)"
mkdir -p "$RESULTS_DIR"

echo "task_id,score,status" > "$RESULTS_DIR/summary.csv"
echo "=== Codex Full Suite Test ===" | tee "$RESULTS_DIR/batch.log"
echo "Started: $(date)" | tee -a "$RESULTS_DIR/batch.log"
echo "" | tee -a "$RESULTS_DIR/batch.log"

# Get all task IDs sorted by timeout (shortest first)
TASKS=$(for f in builtin/tasks/*/task.acl; do
    task=$(basename $(dirname "$f"))
    timeout=$(grep "solution_timeout_sec" "$f" 2>/dev/null | grep -oE '[0-9]+' || echo "300")
    echo "$timeout $task"
done | sort -n | awk '{print $2}')

TOTAL=0
COMPLETED=0
ERRORS=0

for task in $TASKS; do
    TOTAL=$((TOTAL + 1))
    echo "[$(date +%H:%M:%S)] ($TOTAL/52) Running $task..." | tee -a "$RESULTS_DIR/batch.log"
    
    output=$($BENCH run "$task" --agent codex 2>&1)
    exit_code=$?
    
    echo "$output" > "$RESULTS_DIR/${task}.log"
    
    score=$(echo "$output" | grep "COMPLETED" | grep -oE "score=[0-9.]+" | cut -d= -f2)
    
    if [ -n "$score" ]; then
        echo "  -> COMPLETED score=$score" | tee -a "$RESULTS_DIR/batch.log"
        echo "$task,$score,completed" >> "$RESULTS_DIR/summary.csv"
        COMPLETED=$((COMPLETED + 1))
    else
        echo "  -> ERROR (exit=$exit_code)" | tee -a "$RESULTS_DIR/batch.log"
        echo "$output" | tail -3 >> "$RESULTS_DIR/batch.log"
        echo "$task,,error" >> "$RESULTS_DIR/summary.csv"
        ERRORS=$((ERRORS + 1))
    fi
done

echo "" | tee -a "$RESULTS_DIR/batch.log"
echo "=== Summary ===" | tee -a "$RESULTS_DIR/batch.log"
echo "Total: $TOTAL" | tee -a "$RESULTS_DIR/batch.log"
echo "Completed (scored): $COMPLETED" | tee -a "$RESULTS_DIR/batch.log"
echo "Errors: $ERRORS" | tee -a "$RESULTS_DIR/batch.log"
echo "Finished: $(date)" | tee -a "$RESULTS_DIR/batch.log"
