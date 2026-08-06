候选对话历史未持久化，无法事后复盘

标签: enhancement

## 现象

模型候选（model candidate）执行完毕后，其完整对话历史（工具调用、模型回复、中间推理）不被保存。评测结束后无法回溯候选的解题过程，难以诊断失败原因或对比不同模型的行为差异。

## 根因

`execute_candidate` 未将 session 快照和 trajectory 写入磁盘。EdgeBench 对此有 `agent_output.txt` 机制，a3s-bench 缺少等价实现。

## 环境

- a3s-bench v0.1.2
- 任何使用 model candidate 的评测
