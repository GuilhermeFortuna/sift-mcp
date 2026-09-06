# SIFT-015 implementation plan: Phase 1 readiness remediation

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/SIFT-015-phase-1-readiness-remediation-spec.md`](../specs/SIFT-015-phase-1-readiness-remediation-spec.md)  
**Depends on:** SIFT-001, SIFT-014

## Interfaces produced

This task does not change the MCP wire API. It adds a shared retrieval backend
boundary and snapshot-bound read interfaces used by both evaluator and daemon
search, while retaining the existing model and dense-search feature boundaries.

## Ordered implementation

1. Add failing focused tests for shared production/evaluator search semantics,
   all retriever failure combinations, generation-consistent snapshot reads,
   failed-refresh retention, publication recovery, and embedding reuse across
   flush batches. Confirm each relevant failure before implementation.
2. Move embedding, lexical/dense dispatch, failure classification, RRF fusion,
   diagnostics, and result assembly into one retrieval pipeline. Adapt both
   evaluator indexes and daemon snapshots to it, and remove daemon-local
   `search_with_parts`.
3. Remove per-request scoped thread spawning. Execute lexical and dense paths
   sequentially inside the daemon's existing bounded `spawn_blocking` worker.
4. Replace resident record/body maps with a logical immutable `FrozenSearch`
   snapshot containing dense state, a frozen Tantivy generation, snapshot-bound
   SQLite read connections/transactions, model identity, and counters.
5. Keep indexing on the writable store/index without mutating the currently
   served logical snapshot. After store and lexical publication, build and
   validate replacement dense, Tantivy, and SQLite readers, then atomically swap
   the serving `Arc`. Keep the old snapshot serving if refresh fails.
6. Preserve on-demand body/snippet reads from the matching frozen Tantivy
   generation and on-demand metadata reads from snapshot-bound SQLite readers.
   Add the operation-wide content-hash embedding cache if tests show reuse is
   only batch-local; retain one row/location per occurrence.
7. Run focused CPU tests and CUDA compile checks, then broader affected-crate
   tests. Do not run long acceptance, mined-evaluation, or benchmark workloads.
8. Update only SIFT-015 documentation/status as justified by implemented and
   verified behavior; preserve all unrelated UI statuses and do not claim
   human/GPU acceptance.

## Validation

- Use `CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1` for focused and crate tests.
- Run retrieval, daemon, and indexing tests, including their new focused tests.
- Compile `daemon`'s CUDA binary and the CUDA evaluator example without
  executing them.
- Run short live daemon checks with a small repository where practical.
- Do not run Phase 1 acceptance, git-mined evaluation, benchmark suites, or
  long resource/latency runs.

## Handoff

Report files changed, root causes fixed, the shared-pipeline and snapshot
architecture, duplicate-content reuse evidence, focused/live tests, exact
commands, failures, branch-control limitations, and any technically unresolved
items. Human/GPU acceptance remains outstanding.
