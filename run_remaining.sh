#!/bin/bash
cd /home/qmy/projects/a3s-bench
BENCH="./target/release/a3s-bench"
RESULTS_DIR="codex_results_20260724_223520"

TASKS="ffmpeg_swscale_reimplementation git_rewrite_in_zig ann_vector_search_qps smt_solver ordinal_notation_well_foundedness dabic_gravity_inversion"

echo "=== Remaining Tasks Batch ===" | tee -a "$RESULTS_DIR/batch.log"
echo "Started: $(date)" | tee -a "$RESULTS_DIR/batch.log"

for task in $TASKS; do
    echo "[$(date +%H:%M:%S)] Running $task..." | tee -a "$RESULTS_DIR/batch.log"
    
    output=$($BENCH run "$task" --agent codex 2>&1)
    exit_code=$?
    
    echo "$output" > "$RESULTS_DIR/${task}.log"
    
    score=$(echo "$output" | grep "COMPLETED" | grep -oE "score=[0-9.]+" | cut -d= -f2)
    
    if [ -n "$score" ]; then
        echo "  -> COMPLETED score=$score" | tee -a "$RESULTS_DIR/batch.log"
        echo "$task,$score,completed" >> "$RESULTS_DIR/summary.csv"
    else
        echo "  -> ERROR (exit=$exit_code)" | tee -a "$RESULTS_DIR/batch.log"
        echo "$output" | tail -3 >> "$RESULTS_DIR/batch.log"
        echo "$task,,error" >> "$RESULTS_DIR/summary.csv"
    fi
done

echo "=== Remaining Tasks Complete ===" | tee -a "$RESULTS_DIR/batch.log"
echo "Finished: $(date)" | tee -a "$RESULTS_DIR/batch.log"
