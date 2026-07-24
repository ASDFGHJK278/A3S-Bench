Hard link detection in terminal workspace crashes benchmark

<!-- 此问题将在存储库 A3S-Lab/Bench (https://github.com/A3S-Lab/Bench) 中创建。更改此行不起任何作用。 -->

代理人: 
标签: bug
里程碑: 
项目: 



<!-- 编辑新问题的正文，然后单击编辑器右上角的 ✓"创建问题"按钮。第一行将是问题标题。代理人和标签紧跟在空白行后面。在开始问题正文之前留出空行。 -->

## Symptom

During workspace collection, encountering a hard-linked file (nlink > 1) causes the benchmark to terminate immediately with a fatal error.

## Root Cause

`collect_terminal_files()` uses `anyhow::ensure!(metadata.nlink() == 1, ...)` to check for hard links. When the submitted workspace contains a file with nlink > 1, the entire benchmark bails. Hard links are legitimate filesystem behavior on Linux and should not cause a crash.

## Environment

- a3s-bench workspace collection logic (`src/submission.rs`)
