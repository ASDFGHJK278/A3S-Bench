Workspace OCI seed re-extracted on every run, taking minutes for large seeds

Labels: performance

## Symptom

Every `a3s bench run` on a task with a `workspace_seed` performs the full `docker create` → `docker cp` → `tar -x` pipeline to extract the workspace seed. For large seeds (248k files / 14 GB), a single extraction takes several minutes. When batch-evaluating dozens of tasks, repeatedly extracting the same image seed becomes the dominant bottleneck.

## Root cause

`materialize_seed` has no caching layer — every call unconditionally creates a container, extracts content, and sets permissions. The extraction result for a given image_id + source_path + platform combination is fully reusable but is never saved.

## Environment

- a3s-bench v0.1.2
- Docker Runtime
- Large OCI workspace seeds (e.g. openttd_transport_ai and similar tasks)
