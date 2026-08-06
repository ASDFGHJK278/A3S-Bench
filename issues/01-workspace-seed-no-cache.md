Workspace OCI Seed 每次运行都重新提取，大型 seed 耗时数分钟

标签: performance

## 现象

每次 `a3s bench run` 对带有 `workspace_seed` 的任务，都执行完整的 `docker create` → `docker cp` → `tar -x` 流程提取 workspace seed。对于大型 seed（248k 文件 / 14GB），单次提取耗时数分钟。批量评测数十个任务时，重复提取同一镜像的 seed 成为主要瓶颈。

## 根因

`materialize_seed` 无缓存层——每次调用都无条件创建容器、提取内容、设置权限。同一 image_id + source_path + platform 组合的提取结果完全可复用，但没有被保存。

## 环境

- a3s-bench v0.1.2
- Docker Runtime
- 大型 OCI workspace seed（如 openttd_transport_ai 等任务的 seed）
