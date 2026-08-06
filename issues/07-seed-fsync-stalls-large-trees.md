Seed 缓存提取后逐文件 fsync 导致多分钟卡顿

标签: performance

## 现象

Workspace seed 提取后对 tree 中每个文件执行 `fsync`，在 ext4 文件系统上每个 fsync 强制一次 journal commit。对于 248k 文件的 seed，这一步卡顿数分钟。

## 根因

`sync_seed_tree` 递归遍历整个 tree 对每个常规文件调用 `sync_all()`。缓存通过 `.complete` marker 文件验证完整性，即使崩溃丢失也可重新生成，逐文件 fsync 的代价远超其收益。

## 环境

- a3s-bench v0.1.2
- ext4 文件系统
- 大型 workspace seed（100k+ 文件）
