Benchmark runner script swallows a3s bench output

<!-- 此问题将在存储库 A3S-Lab/Bench (https://github.com/A3S-Lab/Bench) 中创建。更改此行不起任何作用。 -->

代理人: 
标签: bug
里程碑: 
项目: 



<!-- 编辑新问题的正文，然后单击编辑器右上角的 ✓"创建问题"按钮。第一行将是问题标题。代理人和标签紧跟在空白行后面。在开始问题正文之前留出空行。 -->

## Symptom

`run_full_benchmark.sh` captures all output from `a3s bench run` into a variable via `$(...)`. The terminal shows no real-time output, making it impossible to tell whether a task is running normally or has hung. No error information is visible when a task fails.

## Root Cause

The script uses `OUTPUT=$(a3s bench run ... 2>&1 | tee -a "$RAW_LOG")`. The `$()` captures all stdout into the variable. Although `tee` writes to a file, nothing is displayed on the terminal. The terminal is completely silent for the entire duration of each task.

## Environment

- a3s-bench helper script (`run_full_benchmark.sh`)
