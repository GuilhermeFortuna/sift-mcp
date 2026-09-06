# SIFT-015: Phase 1 readiness remediation

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Project direction:** [`../../cuda-mcp-rtx2060-plan.md`](../../cuda-mcp-rtx2060-plan.md), [`../../tech-stack.md`](../../tech-stack.md)  
**Depends on:** SIFT-001, SIFT-014  
**Implementation plan:** [`../plans/SIFT-015-phase-1-readiness-remediation-plan.md`](../plans/SIFT-015-phase-1-readiness-remediation-plan.md)

## Purpose

The merged Phase 1 implementation is not yet fit for truthful acceptance
measurement. Evaluation must exercise the production model and dense backend,
index maintenance must reconcile the complete repository state, normal reads
must remain concurrent, and the store and lexical index must recover to one
coherent publication after interruption.

## Requirements

### Truthful retrieval measurement

- The evaluation harness loads the configured production ONNX model and refuses
  to evaluate a store created for a different model.
- Evaluation uses the configured CUDA dense backend, with an explicit feature
  boundary that remains buildable on CPU-only hosts when the example is not run.
- The daemon's CUDA feature enables the retrieval CUDA implementation, and both
  resident loading and rebuild select the CUDA dense backend under that feature.

### Reconciliation and metadata

- A full index tombstones every previously live store file absent from the
  successful repository walk.
- Dirty deletions, excluded files, unsupported files, and unreadable files do
  not leave stale rows behind.
- Identical chunk content occurring in multiple files retains one live row and
  one lexical location per occurrence while sharing embedding computation.

### Serving and publication

- Normal searches use an immutable resident snapshot and do not hold the
  indexing owner lock during inference or retrieval.
- The resident snapshot is logical rather than a full physical corpus copy: it
  retains immutable dense state, a frozen Tantivy generation, snapshot-bound
  SQLite readers, model identity, and lightweight counters only.
- Metadata and bodies are loaded on demand from readers bound to the same
  publication state; a search never mixes dense, SQLite, and Tantivy states
  from different publications.
- A failed replacement refresh leaves the previous valid snapshot serving and
  reports the refresh failure separately.
- Store mutations and lexical mutations are journaled in an ordered,
  crash-recoverable publication protocol, including insertion, removal, rename,
  and compaction row renumbering.
- Recovery is idempotent and never applies a lexical removal or rename unless
  the store mutation is visible.

### Project tracking

- The authoritative task ledger records completed prerequisite work accurately,
  keeps SIFT-012 ready for its measurement, and keeps SIFT-013 blocked until
  SIFT-012 and its required evidence are complete.

## Constraints and non-goals

- No Phase 1 acceptance measurement is performed by this task.
- No GPU benchmark, cold-cache resource measurement, or human acceptance claim
  is inferred from CPU-only or compile-only validation.
- The exhaustive dense search contract and model trait boundary remain intact;
  this task changes backend selection and publication/serving integration.
- Indexing remains incremental and does not copy the full physical store for
  every reindex.
- No runtime Python is introduced.

## Acceptance criteria

### Agent-verifiable

1. The evaluator has no `MockEmbedder` path and its CUDA example compiles.
2. The CUDA daemon compiles with retrieval CUDA enabled.
3. Targeted storage, indexing, and daemon tests pass with bounded build and test
   concurrency.
4. Regression tests cover duplicate locations, full reconciliation, dirty
   deletion, concurrent normal searches, and publication recovery.
5. The status ledger has the dependency-consistent Phase 1 values.

### Human-verifiable

1. SIFT-013 measures retrieval quality with the pinned mined corpus using the
   real model and records the target-GPU latency evidence.
2. The target RTX 2060 run confirms CUDA initialization, dense retrieval, and
   the latency/VRAM criteria under ordinary operating conditions.
