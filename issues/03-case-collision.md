Terminal workspace case-collision check is fatal, causing unnecessary benchmark failures

<!-- 此问题将在存储库 A3S-Lab/Bench (https://github.com/A3S-Lab/Bench) 中创建。更改此行不起任何作用。 -->

代理人: 
标签: bug
里程碑: 
项目: 



<!-- 编辑新问题的正文，然后单击编辑器右上角的 ✓“创建问题”按钮。第一行将是问题标题。代理人和标签紧跟在空白行后面。在开始问题正文之前留出空行。 -->

## 现象

Workspace 最终验证阶段检测到大写字母碰撞的文件名，直接以严重错误终止：



已确认触发的任务：
- schemathesis_config_modernization
- schemathesis_datagen_pipeline

## 根因

Workspace 验证逻辑使用 anyhow::ensure! 宏来检查大小写碰撞。当发现同一目录下存在仅大小写不同的文件名时，代码直接抛出致命错误。这两个任务的工作区中本身包含大小写碰撞的文件名（任务设计上的正常情况），但验证阶段将这种情况视为致命错误。

## 修复建议

建议将大小写碰撞检查由 anyhow::ensure! 改为 warn!，仅在日志中提醒但继续执行。

## 环境

- a3s-bench 框架的 workspace 验证逻辑
