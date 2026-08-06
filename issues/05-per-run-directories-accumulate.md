每次运行的 workspace 和 submission 目录不回收，无限累积

标签: bug

## 现象

`a3s bench run` 在 `.a3s/bench/workspaces/` 和 `.a3s/bench/submissions/` 下按 `{task_id}-{pid}` 创建运行目录，但运行结束后不清理。批量评测后这些目录（包含完整 workspace 和 submission 树）累积到数十 GB，浪费磁盘空间。

## 根因

`execute_inner` 创建 workspace 和 submission 目录后没有回收逻辑。submission 树被 `seal_role_input_tree` 设为只读（0o555），普通的 `remove_dir_all` 会静默失败，需要先恢复写权限。

## 环境

- a3s-bench v0.1.2
- 批量评测场景（数十个任务连续运行）
