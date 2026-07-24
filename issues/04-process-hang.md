Process hangs indefinitely during long-running candidate execution

<!-- 此问题将在存储库 A3S-Lab/Bench (https://github.com/A3S-Lab/Bench) 中创建。更改此行不起任何作用。 -->

代理人: 
标签: bug
里程碑: 
项目: 



<!-- 编辑新问题的正文，然后单击编辑器右上角的 ✓"创建问题"按钮。第一行将是问题标题。代理人和标签紧跟在空白行后面。在开始问题正文之前留出空行。 -->

## Symptom

During long-running candidate model evaluation, the process hangs indefinitely without producing any error output. The benchmark task times out and must be killed manually.

## Root Cause

`output_with_timeout()` uses pipes (`Stdio::piped()`) to read the child process's stdout/stderr. Docker CLI may share the pipe file descriptors with containerd-shim. Even after Docker CLI exits, containerd-shim continues to hold the write end, preventing the parent process's `read_to_end` from ever receiving EOF, causing the process to hang indefinitely.

## Environment

- a3s-bench candidate runner (`src/runtime.rs`)
- Docker CLI + containerd-shim
