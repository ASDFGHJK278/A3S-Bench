#!/bin/bash
cd /home/qmy/projects/a3s-bench
BENCH="./target/release/a3s-bench"
RESULTS_DIR="codex_results_20260724_223520"

# Short fixable tasks first, then the 2 remaining long tasks
TASKS="order_addition_permutation_optimization college_english_exam_bank jagua_nesting_optimization arc_compiler_runtime ordinal_notation_well_foundedness ffmpeg_swscale_reimplementation dabic_gravity_inversion"

echo "=== Final Tasks Batch ===" | tee -a "$RESULTS_DIR/batch.log"
echo "Started: $(date)" | tee -a "$RESULTS_DIR/batch.log"

for task in $TASKS; do
    echo "[$(date +%H:%M:%S)] Running $task..." | tee -a "$RESULTS_DIR/batch.log"
    
    output=$($BENCH run "$task" --agent codex 2>&1)
    exit_code=$?
    
    echo "$output" > "$RESULTS_DIR/${task}.log"
    
    score=$(echo "$output" | grep "COMPLETED" | grep -oE "score=[0-9.]+" | cut -d= -f2)
    
    if [ -n "$score" ]; then
        echo "  -> COMPLETED score=$score" | tee -a "$RESULTS_DIR/batch.log"
        # Update summary CSV (remove old entry if exists, add new)
        grep -v "^$task," "$RESULTS_DIR/summary.csv" > "$RESULTS_DIR/summary.csv.tmp" 2>/dev/null
        mv "$RESULTS_DIR/summary.csv.tmp" "$RESULTS_DIR/summary.csv" 2>/dev/null
        echo "$task,$score,completed" >> "$RESULTS_DIR/summary.csv"
    else
        echo "  -> ERROR (exit=$exit_code)" | tee -a "$RESULTS_DIR/batch.log"
        echo "$output" | tail -3 >> "$RESULTS_DIR/batch.log"
        grep -v "^$task," "$RESULTS_DIR/summary.csv" > "$RESULTS_DIR/summary.csv.tmp" 2>/dev/null
        mv "$RESULTS_DIR/summary.csv.tmp" "$RESULTS_DIR/summary.csv" 2>/dev/null
        echo "$task,,error" >> "$RESULTS_DIR/summary.csv"
    fi
done

echo "=== Final Tasks Complete ===" | tee -a "$RESULTS_DIR/batch.log"
echo "Finished: $(date)" | tee -a "$RESULTS_DIR/batch.log"
