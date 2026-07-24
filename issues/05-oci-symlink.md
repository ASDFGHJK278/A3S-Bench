OCI workspace seed containing symlinks causes benchmark to bail

<!-- 此问题将在存储库 A3S-Lab/Bench (https://github.com/A3S-Lab/Bench) 中创建。更改此行不起任何作用。 -->

代理人: 
标签: bug
里程碑: 
项目: 



<!-- 编辑新问题的正文，然后单击编辑器右上角的 ✓"创建问题"按钮。第一行将是问题标题。代理人和标签紧跟在空白行后面。在开始问题正文之前留出空行。 -->

## Symptom

When the workspace OCI seed contains symbolic links, a3s-bench's safety mechanism refuses to extract it, causing 7 tasks to be unable to run.

Affected tasks:
- arc_compiler_runtime
- carleson_formalization
- flt_regular_formalization
- lean_analysis_proofs
- new_foundations_consistency
- pfr_formalization
- sphere_eversion_formalization

## Root Cause

The OCI images for these tasks contain workspace seeds with symbolic links. a3s-bench's safety mechanism rejects extracting seeds containing symlinks due to potential path traversal, using `anyhow::ensure!` to bail directly.

## Environment

- a3s-bench workspace materialization component (`src/workspace.rs`)
- Embedded workspace seed in OCI images
