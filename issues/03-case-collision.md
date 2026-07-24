Case-collision check in terminal workspace is fatal, causing unnecessary file loss

<!-- 此问题将在存储库 A3S-Lab/Bench (https://github.com/A3S-Lab/Bench) 中创建。更改此行不起任何作用。 -->

代理人: 
标签: bug
里程碑: 
项目: 



<!-- 编辑新问题的正文，然后单击编辑器右上角的 ✓"创建问题"按钮。第一行将是问题标题。代理人和标签紧跟在空白行后面。在开始问题正文之前留出空行。 -->

## Symptom

During workspace collection, when file names differing only in case are detected, the original implementation terminates immediately, result in submitted files being lost, and the judge cannot receive the complete submission.

Confirmed affected tasks:
- schemathesis_config_modernization
- schemathesis_datagen_pipeline

## Root Cause

The workspace collection logic uses a `HashSet` to detect case collisions, treating `Foo.txt` and `foo.txt` as conflicts. However, all judge containers run on `linux/amd64` (a case-sensitive filesystem), where `Foo.txt` and `foo.txt` are two entirely different files that can coexist. There is no need for a case-collision check on a case-sensitive filesystem.

## Environment

- a3s-bench workspace collection logic (`src/submission.rs`)
- All judge containers use platform `linux/amd64`
