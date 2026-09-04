# SIFT-015 implementation plan: Phase 1 readiness remediation

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/SIFT-015-phase-1-readiness-remediation-spec.md`](../specs/SIFT-015-phase-1-readiness-remediation-spec.md)  
**Depends on:** SIFT-001, SIFT-014

## Interfaces produced

This task does not add a public service API. It tightens backend feature
selection, resident ownership, indexing reconciliation, publication recovery,
and the evaluator's runtime contract.

## Ordered implementation

1. Create the branch `SIFT-015-phase-1-readiness-remediation` and register the
   corrective task in the authoritative status table.
2. Add failing regression coverage for real-model evaluation configuration,
   CUDA retrieval selection, duplicate-content locations, full and dirty
   reconciliation, concurrent normal searches, and store/lexical recovery.
   Confirm the relevant failures before implementation.
3. Implement the evaluator and daemon CUDA feature wiring, retaining the CPU
   default for ordinary host validation.
4. Implement immutable ready-state search snapshots and keep mutable indexing
   ownership separate from normal search requests.
5. Reconcile absent and unreadable paths, and preserve one metadata row per
   content occurrence while reusing one embedding per unique content hash.
6. Add the durable publication journal and recovery path around store and
   lexical mutations, including compaction renumbering.
7. Restore the dependency-consistent task statuses and run bounded targeted
   validation for the affected crates and feature combinations.
8. Report the remaining target-GPU and SIFT-013 human evidence without marking
   those measurements complete.

## Validation

- Format the workspace.
- Compile the affected CPU crates with one Cargo build job.
- Compile the evaluator and daemon CUDA feature targets with one Cargo build
  job; do not run them without the required model/GPU assets.
- Run storage, indexing, and daemon tests with one build job and one test
  thread.
- Do not claim SIFT-013 acceptance from these checks.

## Handoff

Report the corrected evaluator invocation, CUDA feature wiring, reconciliation
and duplicate-location guarantees, serving/publication recovery behavior, the
bounded validation commands and results, and the outstanding human acceptance
measurements.
