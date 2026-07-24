chmod: Operation not permitted inside judge container crashes benchmark

<!-- 此问题将在存储库 A3S-Lab/Bench (https://github.com/A3S-Lab/Bench) 中创建。更改此行不起任何作用。 -->

代理人: 
标签: bug
里程碑: 
项目: 



<!-- 编辑新问题的正文，然后单击编辑器右上角的 ✓“创建问题”按钮。第一行将是问题标题。代理人和标签紧跟在空白行后面。在开始问题正文之前留出空行。 -->

## 现象

Judge 容器在执行 chmod 时因权限不足失败：



chmod 失败后 benchmark 直接崩溃退出。

已确认触发的任务：
- integer_compression_codec
- vehicle_routing_time_windows

## 根因

Judge 容器启动时使用了 --cap-drop ALL，丢弃了包括 CAP_FOWNER 在内的所有 Linux capabilities。CAP_FOWNER 是 chmod 系统调用所需的能力。

Judge 调用链为 command && chmod ... 2>/dev/null; python3 judge.py。当容器的 workspace 中包含权限为非默认值的预置文件时，chmod 需要提升权限，但容器已没有 CAP_FOWNER，导致 chmod 失败。

## 环境

- a3s-bench 框架的 Judge Docker 容器
- Docker 启动参数包含 --cap-drop ALL
