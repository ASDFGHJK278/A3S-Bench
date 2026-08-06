dev-qmy 分支相对 origin/main 的改动总结

## 概述

`dev-qmy` 分支在 `origin/main`（v0.1.2）基础上新增 11 个提交，涵盖性能优化、Judge 语义修正、候选日志持久化、配置兼容性增强和 benchmark 批量跑测脚本。以下按类别总结全部改动。

## 一、Workspace OCI Seed 缓存（性能）

**提交**: `8596f7e` `ec4bd62` `69f958d`

### 问题

每次 `a3s bench run` 都要从 Docker 镜像中重新提取 workspace seed（`docker cp` + `tar -x`），对于大型 seed（248k 文件 / 14GB）耗时数分钟，严重拖慢批量评测。

### 改动

- **`src/workspace.rs`**: 新增 seed 缓存层。首次提取后将 tree 存入 `.a3s/bench/workspace-seeds/<sha256>` 缓存目录，后续命中缓存时用 `cp -a` 克隆，跳过 Docker 提取。
  - `seed_cache_key`: 用 image_id + source_path + platform 计算 SHA-256 缓存键
  - `valid_seed_cache`: 校验 `.complete` marker 文件确认缓存完整性
  - `populate_seed_staging`: 提取到 staging 目录后原子 rename 发布，避免半成品缓存
  - `sweep_stale_staging`: 清理被 kill 遗留的临时 staging 目录
  - `clone_tree`: 用 `cp -a` 替代逐文件 `std::fs::copy`，大幅提速
  - 缓存命中时跳过 `set_tree_owner_only` 遍历（`cp -a` 保留权限）
- **`src/state_fs.rs`**: 新增 `remove_tree`（恢复 sealed 目录可写权限后删除）、`create_unique_staging_directory`、`read_optional_regular_file`、`secure_atomic_write` 等辅助函数
- **`src/config.rs`**: `run_directory` 改为接收 `state_root` 参数，避免重复调用 `state_root()`
- 移除 `sync_seed_tree`（逐文件 fsync 在 ext4 上导致多分钟卡顿），改为依赖 `.complete` marker + 目录 sync

### 效果

大型 seed 首次提取后，后续运行从数分钟降至秒级。

## 二、Judge 超时语义修正

**提交**: `5c9b284`

### 问题

`origin/main` 的 `abnormal_judge_exit` 把 exit code 124（timeout）与 signal kill（OOM/SIGTERM）一同视为基础设施失败，直接 `bail!`。但超时意味着候选代码太慢、Judge 在完整超时窗口内未能完成——这是候选质量问题，不应报错。

### 改动

- **`src/legacy_judge.rs`**: 从 `abnormal_judge_exit` 中移除 `124`，超时不再 bail，而是打印警告并走正常打分路径（`parse_score` 在无结构化结果时返回 0.0）
- 更新测试 `judge_exit_classification_separates_candidate_and_infrastructure_failures`：`Some(124)` 现在断言为非异常

## 三、Raw Score 无 Rescale 配置时的归一化

**提交**: `42c8f6d`

### 问题

`normalize_raw` 在无 rescale spec 时直接 `clamp(0.0, 1.0)` 返回，但部分 Judge 返回 0-100 范围的原始分数，导致得分被错误截断为 1.0。

### 改动

- **`src/legacy_judge.rs`**: 无 spec 时改为 `clamp(0.0, 100.0) / 100.0`，正确归一化到 0-1

## 四、候选对话日志持久化

**提交**: `5c9b284`

### 改动

- **`src/bench_run.rs`**: 新增 `TransientRunDirs` RAII guard，在结果持久化后自动回收 workspace 和 submission 目录；新增 `run_id` + `state_root` 参数传递
- **`src/model_candidate.rs`**: `ModelCandidateRequest` 新增 `log_dir` 字段，将完整候选对话（session 快照 + JSONL trajectory）持久化到 `.a3s/bench/runs/<run_id>/`，对标 EdgeBench 的 `agent_output.txt`
- **`src/bench_run/tests.rs`**: 更新测试调用签名

## 五、配置兼容性

**提交**: `42c8f6d`

### 改动

- **`src/config.rs`**: `resolve_model_route` 在 `api_key`/`base_url` 查找失败时回退到 camelCase 变体 `apiKey`/`baseUrl`，兼容不同 provider 配置风格

## 六、Benchmark 批量跑测脚本

**提交**: `2408851` `2bc299e` `e24c0ca` `3b5c814` `8c350eb`

### 改动

- **`run_all_tasks.sh`**: 初版批量评测脚本，逐任务运行并汇总 CSV
- **`run_full_benchmark.sh`**: 改进版，支持 `--json` 输出 + Python 解析、灵活的任务选择（序号/范围/名称混合）、逐任务日志和汇总
- **`.gitignore`**: 排除 benchmark 产出文件（`*.json`、`*.log`、`*.csv`、`*.tmp`、config.acl 等）

## 七、AGENTS.md

**提交**: `d70eb0c`

新增仓库级 agent 指引文件，描述项目结构、构建命令、编码规范、测试指南和提交规范。

## 验证

- `cargo test --locked`: 106 passed, 0 failed, 4 ignored
- `cargo fmt --all -- --check`: 通过
- `cargo clippy --locked --all-targets -- -D warnings`: 通过
- `dev-qmy` 干净领先 `origin/main` 11 个提交，无分叉
