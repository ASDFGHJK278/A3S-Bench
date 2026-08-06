无 rescale 配置时 raw score 被错误截断为 1.0

标签: bug

## 现象

部分 Judge 返回 0-100 范围的原始分数。当任务没有 rescale 配置时，`normalize_raw` 直接执行 `raw.clamp(0.0, 1.0)`，所有 >1 的分数被截断为 1.0，导致候选得分严重失真。

## 根因

`normalize_raw` 在 `spec` 为 `None` 的分支假设 raw score 已在 0-1 范围内，但实际有 Judge 返回百分制分数。缺少从 0-100 到 0-1 的归一化。

## 环境

- a3s-bench v0.1.2
- 无 rescale spec 的任务 + 返回百分制分数的 Judge
