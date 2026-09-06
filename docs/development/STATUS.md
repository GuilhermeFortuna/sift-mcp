# Task status

Authoritative status for every SIFT task. Specs and plans link here; they
never restate a status value.


| Status    | Meaning                                                         |
| --------- | --------------------------------------------------------------- |
| `BLOCKED` | A dependency is not yet `DONE`, or the spec/plan pair is absent |
| `READY`   | Dependency satisfied, spec and plan written, not yet complete   |
| `DONE`    | Validation passed, handoff reported, acceptance recorded        |


Project direction: `[../cuda-mcp-rtx2060-plan.md](../cuda-mcp-rtx2060-plan.md)`
and `[../tech-stack.md](../tech-stack.md)`. Where the two disagree on language
and runtime, `tech-stack.md` governs: the runtime is a Rust workspace and Python
appears only in `tools/`.


| ID                                                                                                                              | Batch | Status    | Depends on                   | Deliverable                                                                                                      |
| ------------------------------------------------------------------------------------------------------------------------------- | ----- | --------- | ---------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| [SIFT-001](specs/SIFT-001-workspace-skeleton-spec.md) / [Plan](plans/SIFT-001-workspace-skeleton-plan.md)                       | 01    | `DONE`    | None                         | A Cargo workspace whose crates all build empty, and a single validation command every later task must pass       |
| [SIFT-002](specs/SIFT-002-chunk-store-spec.md) / [Plan](plans/SIFT-002-chunk-store-plan.md)                                     | 01    | `DONE`    | SIFT-001                     | Durable chunk metadata in SQLite paired with an fp16 embedding matrix on disk, with tombstones and compaction    |
| [SIFT-003](specs/SIFT-003-symbol-chunking-spec.md) / [Plan](plans/SIFT-003-symbol-chunking-plan.md)                             | 01    | `DONE`    | SIFT-001                     | Source files turned into symbol-aligned chunk records with stable content hashes, with excluded paths never read |
| [SIFT-004](specs/SIFT-004-model-export-spec.md) / [Plan](plans/SIFT-004-model-export-plan.md)                                   | 01    | `DONE`    | SIFT-001                     | An fp16 ONNX embedding model plus a committed reference-vector fixture that pins correct output                  |
| [SIFT-005](specs/SIFT-005-embedding-inference-spec.md) / [Plan](plans/SIFT-005-embedding-inference-plan.md)                     | 01    | `DONE`    | SIFT-004                     | Batched GPU embedding behind a trait, numerically matching the reference fixture, with a CPU mock for CI         |
| [SIFT-006](specs/SIFT-006-repository-indexing-spec.md) / [Plan](plans/SIFT-006-repository-indexing-plan.md)                     | 01    | `DONE`    | SIFT-002, SIFT-003, SIFT-005 | A repository indexed end to end, and re-indexed after a commit in time proportional to the diff                  |
| [SIFT-007](specs/SIFT-007-lexical-retrieval-spec.md) / [Plan](plans/SIFT-007-lexical-retrieval-plan.md)                         | 01    | `DONE`    | SIFT-002, SIFT-003           | BM25 ranking over chunks that finds exact identifiers and error strings                                          |
| [SIFT-008](specs/SIFT-008-dense-retrieval-spec.md) / [Plan](plans/SIFT-008-dense-retrieval-plan.md)                             | 01    | `DONE`    | SIFT-002, SIFT-005           | Exhaustive nearest-neighbour ranking over the whole matrix, with no approximation anywhere                       |
| [SIFT-009](specs/SIFT-009-fusion-and-results-spec.md) / [Plan](plans/SIFT-009-fusion-and-results-plan.md)                       | 01    | `DONE`    | SIFT-007, SIFT-008           | One fused ranking from both retrievers, rendered as triage-sufficient result records                             |
| [SIFT-010](specs/SIFT-010-resident-daemon-spec.md) / [Plan](plans/SIFT-010-resident-daemon-plan.md)                             | 01    | `DONE`    | SIFT-006, SIFT-009           | A daemon holding models and indexes resident behind a unix socket, auto-started and idle-evicting                |
| [SIFT-011](specs/SIFT-011-mcp-tool-surface-spec.md) / [Plan](plans/SIFT-011-mcp-tool-surface-plan.md)                           | 01    | `DONE`    | SIFT-010                     | The four Phase 1 MCP tools served by a thin stdio client that starts in milliseconds                             |
| [SIFT-012](specs/SIFT-012-git-mined-eval-spec.md) / [Plan](plans/SIFT-012-git-mined-eval-plan.md)                               | 01    | `READY`   | SIFT-006, SIFT-009           | Thousands of retrieval labels mined from a pinned third-party history, and a metrics report over them            |
| [SIFT-013](specs/SIFT-013-phase-1-acceptance-spec.md) / [Plan](plans/SIFT-013-phase-1-acceptance-plan.md)                       | 01    | `BLOCKED` | SIFT-011, SIFT-012           | A measured verdict on the Phase 1 exit criteria, and a locked baseline for Phase 2                               |
| [SIFT-014](specs/SIFT-014-ci-resource-bounds-spec.md) / [Plan](plans/SIFT-014-ci-resource-bounds-plan.md)                       | 01    | `DONE`    | SIFT-001                     | Conservative validation defaults that keep large test artifacts from exhausting developer workstations           |
| [SIFT-015](specs/SIFT-015-phase-1-readiness-remediation-spec.md) / [Plan](plans/SIFT-015-phase-1-readiness-remediation-plan.md) | 01    | `READY`   | SIFT-001, SIFT-014           | Corrective hardening required before truthful Phase 1 acceptance measurement                                     |


Batch 01 implements Phase 1 of the project direction: the retrieval foundation.
It is driven by the git-mined evaluation harness built in SIFT-012 and measured
against the three Phase 1 exit criteria — Top-3 accuracy at or above 0.80,
`search_code` p95 latency under 400 ms end to end, and cold agent start under
200 ms. SIFT-013 records those measurements and locks them as the baseline that
Phase 2 reranking must beat; no batch 02 task may be written before that
baseline exists, because Phase 2 is gated on the margin it shows.

Evaluation corpora are split by what each can support. The mined label set comes
from a pinned third-party checkout — `~/llama.cpp`, measured at 10,688 commits
and roughly a 47% filter survival rate, so several thousand labels — because the
first-party repositories here total about 65 labels after filtering, an interval
too wide to judge the 0.80 top-3 target against. The documentation-derived and
hand-written sets come from first-party repositories instead, since they cover
the languages this project will actually be pointed at and scale with symbol
count rather than commit count. Results are reported per corpus, never merged.
No third-party content is committed here; labels regenerate from the pinned
revision.

## UI series — Batch 01: Local console

The UI series is separately authorized and uses its own `UI-` prefix and Batch
01. It is not SIFT Batch 02 and does not depend on or authorize Phase 2 retrieval
planning. The existing SIFT-013 baseline gate and all hardware/human acceptance
requirements remain unchanged. Cross-series dependencies below are deliberate;
planning the UI series does not authorize completing SIFT remediation tasks.


| ID                                                                                                              | Batch | Status    | Depends on                   | Deliverable                                                                                                      |
| --------------------------------------------------------------------------------------------------------------- | ----- | --------- | ---------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| [UI-001](specs/UI-001-daemon-observability-spec.md) / [Plan](plans/UI-001-daemon-observability-plan.md)         | 01    | `DONE`    | SIFT-010, SIFT-011, SIFT-015 | Passive daemon observation, safe request events, indexing snapshots, and truthful resource measurements          |
| [UI-002](specs/UI-002-console-service-spec.md) / [Plan](plans/UI-002-console-service-plan.md)                   | 01    | `DONE`    | UI-001                       | Local Rust HTTP service, repository registry, bounded history, action API, live updates, and frontend foundation |
| [UI-003](specs/UI-003-observability-ui-spec.md) / [Plan](plans/UI-003-observability-ui-plan.md)                 | 01    | `READY`   | UI-002                       | Shared application shell, Overview, and Activity with accessible live diagnostic views                           |
| [UI-004](specs/UI-004-repository-operations-ui-spec.md) / [Plan](plans/UI-004-repository-operations-ui-plan.md) | 01    | `BLOCKED` | UI-003                       | Registration, honest freshness inspection, explicit start/update/rebuild, and durable progress views             |
| [UI-005](specs/UI-005-search-lab-ui-spec.md) / [Plan](plans/UI-005-search-lab-ui-plan.md)                       | 01    | `BLOCKED` | UI-004                       | Query/snippet search, symbol inspection, complete-console validation, and local distribution                     |


UI task branches use `UI-NNN-slug`, one implementation task per branch. Every
task has a code-free spec and an implementation-free plan. UI-001 cannot begin
until SIFT-015 meets this ledger's dependency contract. Owner-only acceptance
is recorded explicitly rather than inferred from CPU fixtures or compilation.
