Process hangs indefinitely during long-running candidate execution

<!-- 此问题将在存储库 A3S-Lab/Bench (https://github.com/A3S-Lab/Bench) 中创建。更改此行不起任何作用。 -->

代理人: 
标签: bug
里程碑: 
项目: 



<!-- 编辑新问题的正文，然后单击编辑器右上角的 ✓“创建问题”按钮。第一行将是问题标题。代理人和标签紧跟在空白行后面。在开始问题正文之前留出空行。 -->

## 现象

在长时间运行的候选模型评测中，进程会无限挂起而不产生任何错误信息，导致 benchmark 当前任务超时。

## 根因

问题发生在 Rust 的 std::process::Command 通过管道读取子进程的 stdout/stderr 时。当子进程的输出缓冲区被填满时：

1. 子进程因为管道的读端未被读取而阻塞在写入操作上
2. 父进程在等待子进程结束，而没有同步读取管道

这导致父子进程互相等待的死锁。

## 环境

- a3s-bench 框架的 candidate runner
- Rust 的 std::process::Command 实现
- 子进程产生大量输出的场景
