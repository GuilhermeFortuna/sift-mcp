# SIFT-007: Lexical retrieval

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Project direction:** [`../../cuda-mcp-rtx2060-plan.md`](../../cuda-mcp-rtx2060-plan.md), [`../../tech-stack.md`](../../tech-stack.md)  
**Depends on:** SIFT-002, SIFT-003  
**Implementation plan:** [`../plans/SIFT-007-lexical-retrieval-plan.md`](../plans/SIFT-007-lexical-retrieval-plan.md)

## Purpose

Dense retrieval underperforms keyword search on exactly the queries agents ask
most often: a specific identifier, a literal error string, a symbol name copied
from a stack trace. An embedding of `EAGAIN` is not meaningfully closer to the
code that returns it than to any other error-handling code, whereas a term match
is decisive. This task provides keyword ranking over the same chunks the dense
path sees, so the fusion in SIFT-009 has a genuinely complementary signal rather
than two views of the same similarity. The project direction places both paths
in Phase 1 for this reason.

## Requirements

### Query behaviour

- An exact identifier appearing in a chunk's body, signature, or symbol name
  ranks that chunk highly, and identifiers written in camel case, snake case, or
  with namespace separators are matched when the query uses a different casing
  or splits them differently.
- A literal string from an error message or log line retrieves the chunk
  containing it.
- A multi-word natural-language query returns results ranked by term relevance
  rather than requiring every term to be present.
- A query matching nothing returns an empty result, not an error and not an
  arbitrary low-scoring set.
- Ranking is deterministic: the same query against the same index returns the
  same order, with ties broken by a stated rule rather than by iteration order.

### Correspondence with the store

- Every result identifies the chunk by the position the store assigned, so a
  result can be resolved to metadata without a second lookup path.
- Chunks removed from the store are absent from results; the two never disagree
  about what exists.
- The lexical index is updated by the same operations that update the store, so
  no separate reindexing step can be forgotten.

### Score semantics

- Each result carries a score whose meaning and range are documented, and the
  documentation states plainly whether scores are comparable across queries.
- Results are returned in descending score order and the count returned is
  bounded by a caller-supplied limit.

### Cost

- A query over a full-size index completes within the lexical portion of the
  project direction's latency budget, measured rather than assumed.
- Opening the index is fast enough to be done at daemon start without dominating
  it, and the index does not need to be resident in memory in full.
- Indexing a chunk costs time proportional to its size, so incremental updates
  stay proportional to the diff.

## Constraints and non-goals

- No fusion with dense results. SIFT-009 fuses; producing "just a quick combined
  score" here would put the fusion policy in two places.
- No reranking, no cross-encoder, no learned scoring. Phase 2 and gated.
- No embeddings, no similarity. This path is lexical by definition.
- No query rewriting, expansion, synonyms, or spelling correction. If retrieval
  quality needs those, the evaluation harness should say so first; guessing now
  adds untestable behaviour.
- No regex or glob search surface. The agent already has grep, and the project
  direction requires every component to beat a baseline the agent has.
- No stemming tuned per natural language. Code identifiers, not prose corpora.
- No result snippet extraction or highlighting. Result presentation belongs to
  SIFT-009, so that both retrieval paths present identically.

## Acceptance criteria

### Agent-verifiable

1. A fixture index returns the chunk containing a unique identifier at rank one,
   for that identifier written in its original casing and in at least two other
   splittings.
2. A literal error string present in exactly one chunk retrieves that chunk at
   rank one.
3. A multi-word query returns results ranked by relevance, with a snapshot test
   pinning the order for a fixture corpus.
4. A query matching nothing returns an empty result and no error.
5. Repeating a query returns an identical ordering, including for results with
   equal scores.
6. Every returned position resolves to a live chunk in the store; removing a
   chunk removes it from subsequent results without a separate reindex call.
7. The returned count never exceeds the caller's limit, and results are in
   descending score order.
8. Score semantics and range are documented and asserted by a test that would
   fail if the scoring scheme changed.
9. The full validation suite passes: `./ci.sh`

### Human-verifiable

1. Query latency over an index of at least 100,000 chunks is measured over 200
   queries after warm-up, and median and 95th percentile are reported against
   the lexical budget.  
   Command: `cargo run --release -p retrieval --example bench_lexical -- <store-path> --queries 200`
2. Index open time and on-disk size at that scale are measured and reported.  
   Command: `cargo run --release -p retrieval --example bench_lexical -- <store-path> --open-only`
