Per-file fsync after seed extraction stalls for minutes on large trees

Labels: performance

## Symptom

After extracting a workspace seed, the code calls `fsync` on every file in the tree. On ext4 filesystems, each `fsync` forces a journal commit. For a seed with 248k files, this step stalls for several minutes.

## Root cause

`sync_seed_tree` recursively traverses the entire tree and calls `sync_all()` on every regular file. The cache is validated by a `.complete` marker file and can be regenerated if lost to a crash, so the cost of per-file fsync far exceeds its benefit.

## Environment

- a3s-bench v0.1.2
- ext4 filesystem
- Large workspace seeds (100k+ files)
