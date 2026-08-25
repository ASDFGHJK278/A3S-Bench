# Issue #28: Order Addition Judge image pins a stale score-helper hash

## Summary

The pinned official EdgeBench Judge image for
`order_addition_permutation_optimization` is internally inconsistent. Its
private score helper has one SHA-256 digest, while the test shipped in the same
image requires a different digest. A valid submission can therefore receive a
nonzero `TOTAL_SCORE` but still make pytest exit with one failed test.

This mismatch is present inside the referenced EdgeBench OCI image. A3S-Bench
does not rebuild or modify that image, so the underlying defect is upstream
artifact drift rather than an A3S-generated Judge change.

## Pinned upstream inputs

- EdgeBench dataset commit:
  `47846a4c3669ad447e0ea984833b0d352460c5f9`
- EdgeBench harness commit:
  `f59bcb0f024d4bc8baedeac271306050e4bb0d33`
- Judge image:
  `seededge/edgebench.judge.order_addition_permutation_optimization:f6f385925889`
- Judge project root:
  `/home/workspace/complex_job_scheduling`

## Evidence

The image's test file
`tests/test_final_result.py` declares:

```text
EXPECTED_SCORE_HELPER_SHA256=
3023b9a449119e862d4ca86d3ab45599e2496e182be82959a642522f915dbbac
```

The helper actually shipped at `tests/score_vcpom_result.py` hashes to:

```text
337837af7067b3dae8d4ef068d26d8dd8ff779f9a627d95451af2ca411c99630
```

Formal run `local-1787296374216-272428-0` demonstrated the impact:

```text
test_no_private_cost_module_access PASSED
test_private_judge_files_are_available FAILED
test_baseline_file_was_not_modified PASSED
test_final_result_exists_and_has_required_fields PASSED
test_final_result_is_valid_permutation_and_cost TOTAL_SCORE 24.8627208316
PASSED
```

The only failure was the stale expected helper digest. The recorded A3S score
was `0.24862720831600002`, and the solution verdict remained `valid`, but the
Judge process exited with status 1.

## Root cause and attribution boundary

The score helper bytes and their integrity constant were not updated together
when the image was assembled. The published image alone does not establish
which source commit or author changed the helper, so this issue must not claim
a specific culprit without upstream build provenance.

What can be established is:

1. Both conflicting artifacts are shipped in the official EdgeBench Judge
   image.
2. The pinned EdgeBench task command runs that image directly.
3. A3S-Bench references the image and copies the submission into its original
   project root; it does not replace the private score helper.

## Impact

- Every otherwise valid submission is reported as 4 passed and 1 failed.
- Judge exit status and pass rate become misleading even when scoring succeeds.
- Systems that require a clean Judge exit can reject or zero a valid result.
- Re-running the Candidate cannot resolve the mismatch because it is entirely
  inside the private Judge image.

## Current A3S-Bench compatibility adaptation

Before running the original pytest command, the generated Judge command edits
only the stale digest constant in the image's ephemeral test file. The
adaptation:

- requires the old digest to occur exactly once and fails closed otherwise;
- replaces it with the digest of the helper actually shipped in the image;
- keeps `/home/workspace/complex_job_scheduling` as the execution directory;
- keeps `python -m pytest tests/test_final_result.py -s -v` unchanged;
- does not modify the helper, submission, scoring formula, or host project.

A direct regression against the official image completed with all five tests
passing and pytest still reported the original project root.

This is a compatibility workaround, not a correction to EdgeBench itself.

## Desired upstream fix

EdgeBench should publish a Judge artifact in which the expected digest matches
the private helper that is actually shipped. The source test, helper, image tag
and build provenance should be updated atomically so consumers do not need to
patch private test code at runtime.

## Acceptance criteria

- The official Judge image is internally hash-consistent without runtime edits.
- The five official tests pass for a known-valid `final_result.txt`.
- The original project root and pytest command remain unchanged.
- The raw `TOTAL_SCORE` is unchanged by the integrity fix.
- A3S-Bench can remove its Order Addition command adaptation after pinning the
  corrected upstream artifact.
