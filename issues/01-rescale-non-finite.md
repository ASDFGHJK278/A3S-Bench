Score normalization exception causes benchmark crash

<!-- 此问题将在存储库 A3S-Lab/Bench (https://github.com/A3S-Lab/Bench) 中创建。更改此行不起任何作用。 -->

代理人: 
标签: bug
里程碑: 
项目: 



<!-- 编辑新问题的正文，然后单击编辑器右上角的 ✓“创建问题”按钮。第一行将是问题标题。代理人和标签紧跟在空白行后面。在开始问题正文之前留出空行。 -->

## 现象

Judge 的分数归一化函数 normalize_raw() 对 0 分进行了自然对数运算，产生 -inf，导致报错：

\Judge rescale produced a non-finite value
benchmark 因此崩溃退出，任务无法完成。

已确认触发的任务：
- exchange_core_throughput
- openttd_transport_ai

## 根因

Judge 返回的 raw_score 为 0 时，normalize_raw() 在 log_max 策略下未正确处理 0 值，导致 math.log(0) 产生 -inf。上层 rescale 逻辑发现非有限值，直接 panic 退出。

## 环境

- a3s-bench 框架内置 Judge 系统
- judge_model: openai/glm-5.2
- 配置：.a3s/config.acl
