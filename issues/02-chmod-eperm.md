chmod failure inside judge container crashes benchmark

<!-- 此问题将在存储库 A3S-Lab/Bench (https://github.com/A3S-Lab/Bench) 中创建。更改此行不起任何作用。 -->

代理人: 
标签: bug
里程碑: 
项目: 



<!-- 编辑新问题的正文，然后单击编辑器右上角的 ✓"创建问题"按钮。第一行将是问题标题。代理人和标签紧跟在空白行后面。在开始问题正文之前留出空行。 -->

## Symptom

The judge container fails to execute `chmod` due to insufficient permissions. Because the command chain uses `&&`, the chmod failure aborts the entire chain, the judge script never runs, and the benchmark exits with an error.

Confirmed affected tasks:
- integer_compression_codec
- vehicle_routing_time_windows

## Root Cause

The judge container is started with `--cap-drop ALL`, dropping all Linux capabilities including `CAP_FOWNER`, which is required for the `chmod` syscall.

The judge invocation chain is `cp -R ... && chmod -R u+rwX ... && python3 judge.py`. When the workspace contains pre-set files with non-default permissions, `chmod` requires `CAP_FOWNER` but the container does not have it. Since the commands are linked with `&&`, the chmod failure aborts the entire chain and the judge never executes.

## Environment

- a3s-bench judge Docker container (`src/legacy_judge.rs`)
- Docker launch parameters include `--cap-drop ALL`
