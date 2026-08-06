Judge 超时被误判为基础设施失败，导致整轮评测中断

标签: bug

## 现象

Judge 进程因 `timeout` 超时退出（exit code 124）时，`abnormal_judge_exit` 将其与 OOM kill（signal 终止）归为同一类异常，直接 `bail!` 报错。整轮 benchmark 中断，后续任务无法继续。

## 根因

`abnormal_judge_exit` 的匹配模式为 `None | Some(124 | 137 | 143)`，把 124（timeout）与 137/143（signal kill）混为一谈。但超时的语义是候选代码太慢、Judge 在完整超时窗口内未能完成——这是候选质量问题，不是基础设施故障。正确的做法是给 0 分并继续下一个任务。

## 环境

- a3s-bench v0.1.2
- Judge 超时窗口较短的任务（候选代码执行缓慢时触发）
