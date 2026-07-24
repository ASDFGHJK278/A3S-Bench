OCI workspace seed containing symlinks causes benchmark to bail

<!-- 此问题将在存储库 A3S-Lab/Bench (https://github.com/A3S-Lab/Bench) 中创建。更改此行不起任何作用。 -->

代理人: 
标签: bug
里程碑: 
项目: 



<!-- 编辑新问题的正文，然后单击编辑器右上角的 ✓“创建问题”按钮。第一行将是问题标题。代理人和标签紧跟在空白行后面。在开始问题正文之前留出空行。 -->

## 现象

当 workspace OCI seed 中包含符号链接时，a3s-bench 的安全机制拒绝解压，导致 7 个任务无法运行：



受影响的任务：
- arc_compiler_runtime
- carleson_formalization
- flt_regular_formalization
- lean_analysis_proofs
- new_foundations_consistency
- pfr_formalization
- sphere_eversion_formalization

## 根因

这些任务的 OCI 镜像中的 workspace seed 包含 symbolic link，来自上游 EdgeBench 镜像构建。a3s-bench 的安全机制为 symlink 可能导致的路径穿越问题而禁止解压含 symlink 的 seed 文件。

## 修复

已在 commit a48c62f 中修复：将 symlink 解压为普通文件（复制目标文件内容），而不是拒绝解压。

## 环境

- a3s-bench 框架的 workspace materialization 组件
- OCI 镜像中的内嵌 workspace seed
