# SIFT-008: Dense retrieval

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Project direction:** [`../../cuda-mcp-rtx2060-plan.md`](../../cuda-mcp-rtx2060-plan.md), [`../../tech-stack.md`](../../tech-stack.md)  
**Depends on:** SIFT-002, SIFT-005  
**Implementation plan:** [`../plans/SIFT-008-dense-retrieval-plan.md`](../plans/SIFT-008-dense-retrieval-plan.md)

## Purpose

Keyword search cannot answer "where do we clamp timestamps that go backwards"
when the code never uses those words. That is the query class the embedding
matrix exists for, and nothing yet reads it. The project direction's central
storage bet is that at this corpus size an exhaustive multiplication of the
whole matrix by the query is both faster than an approximate index and exactly
correct, so this task must deliver a true nearest-neighbour ranking and must
demonstrate the speed claim rather than assume it. If the measurement fails, the
right response is to record it and revisit the bet, not to reach for an
approximate index.

## Requirements

### Ranking

- Every live embedding participates in every query; no candidate is pruned,
  sampled, or clustered away.
- Results are ranked by the similarity the embedding model was trained for, with
  the choice stated, and the ranking is verifiably identical to a naive
  reference computation over the same data.
- Dead positions never appear in results, and the exclusion does not require
  reading the metadata store for every candidate.
- The returned count is bounded by a caller-supplied limit and results are in
  descending similarity order, with ties broken by a stated rule.
- Results identify chunks by store position, matching the lexical path, so the
  two can be fused without translation.

### Model agreement

- A query is refused if it was produced by a different model or has a different
  width than the matrix records, because a silently mismatched query returns
  confident nonsense.
- The query is embedded through the same abstraction the index was built with,
  including the instruction prefix convention if the model has one.

### Numerical behaviour

- Accumulation is done at a precision sufficient that half-precision storage
  does not reorder the top results relative to a full-precision reference, and
  the check that this holds is part of the test suite.
- Similarity scores are documented as to range and meaning, and are stable
  across runs for the same inputs.

### Cost

- A query over a full-size matrix completes within the dense portion of the
  project direction's latency budget, measured on the target hardware.
- Query cost does not include loading or copying the whole matrix per query; the
  matrix is prepared once and reused.
- Memory used by the search path is bounded and reported, and is accounted
  against the same VRAM budget the embedding model occupies.
- The path degrades predictably as the corpus grows: cost as a function of chunk
  count is measured at several sizes rather than at one.

## Constraints and non-goals

- No approximate nearest neighbour, no FAISS, no HNSW, no vector database, no
  clustering or inverted-file structure. The project direction rules these out
  below roughly two million chunks. This is the single strongest temptation in
  the task and it is refused: exhaustive is correct and, at this size, faster.
- No quantization below the stored half precision, no product quantization, no
  dimensionality reduction.
- No fusion with lexical results and no reranking. SIFT-009 and Phase 2.
- No caching of query results. Query text repeats rarely, and a cache would mask
  the latency the SLO is measured against.
- No custom GPU kernels. The project direction forbids starting there; a
  library-provided multiplication is the starting point and only profiling
  justifies more.
- No filtering by language, path, or repository within this task. Filtered
  search is a plausible next feature and it is deferred, because filtering
  interacts with exhaustiveness and needs its own design.
- No incremental index structure to maintain. Appending a row is all the
  maintenance there is, and that is the point of the design.

## Acceptance criteria

### Agent-verifiable

1. Against a fixture matrix, ranking matches a naive reference implementation
   exactly in the identity and order of the top results.
2. With deterministic mock embeddings, a query constructed to be nearest to a
   known chunk returns that chunk at rank one.
3. Half-precision storage is shown not to reorder the top results relative to a
   full-precision reference over a randomized corpus, across many trials.
4. A query of the wrong width, or one tagged with a different model identity, is
   refused with a distinguishable error.
5. Dead positions are absent from results after deletion, without a per-candidate
   metadata lookup.
6. The returned count respects the caller's limit, ordering is descending, and
   repeated queries return identical orderings including ties.
7. Every returned position resolves to a live chunk in the store.
8. The full validation suite passes: `./ci.sh`

### Human-verifiable

1. Query latency is measured on the target GPU over 200 queries after warm-up at
   corpus sizes of roughly 10,000, 50,000, and 200,000 chunks, and median and
   95th percentile at each size are reported against the dense budget.  
   Command: `cargo run --release -p retrieval --example bench_dense -- --sizes 10000,50000,200000 --queries 200`
2. Peak GPU memory attributable to dense search at the largest size is measured
   with the embedding model also resident, and reported against the budget.  
   Command: `cargo run --release -p retrieval --example bench_dense -- --sizes 200000 --report-vram`
3. The exhaustive-search bet is judged against the measurements: the reported
   figures either clear the budget at 200,000 chunks or are recorded as the
   evidence that the storage design needs revisiting.  
   Command: `cargo run --release -p retrieval --example bench_dense -- --sizes 200000 --queries 200`
