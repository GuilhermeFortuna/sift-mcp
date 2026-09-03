# SIFT-009: Fusion and result assembly

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Project direction:** [`../../cuda-mcp-rtx2060-plan.md`](../../cuda-mcp-rtx2060-plan.md), [`../../tech-stack.md`](../../tech-stack.md)  
**Depends on:** SIFT-007, SIFT-008  
**Implementation plan:** [`../plans/SIFT-009-fusion-and-results-plan.md`](../plans/SIFT-009-fusion-and-results-plan.md)

## Purpose

Two rankings exist and neither is the answer. Their scores are not comparable —
one is a term-frequency statistic, the other a similarity — so any attempt to
add or average them encodes an arbitrary exchange rate that will be wrong for
some queries. Fusing by rank rather than by score avoids that, and is what the
project direction specifies. The second half of the problem is what a result
looks like: a file and a line range makes the agent read the whole file to
decide whether the hit is relevant, which spends exactly the tokens this project
exists to save. This task produces one ranking and renders it as records an
agent can triage without opening anything.

## Requirements

### Fusion

- The two retrievers are queried for candidates and their results combined by
  rank position, not by raw score, so that no comparability between the two
  scoring schemes is assumed.
- A chunk found by both retrievers ranks above a chunk of similar standing found
  by only one.
- A chunk found by only one retriever remains eligible for the final results; a
  fusion that can only return the intersection would lose the exact-identifier
  hits the lexical path exists for.
- The candidate depth taken from each retriever, and the fusion's constant, are
  configurable and their default values recorded with the reason for each.
- Fusion is deterministic, with ties broken by a stated rule.
- If one retriever fails or returns nothing, the fused result is the other's,
  and the degradation is visible to the caller rather than silent.

### Result records

- Each result carries file path, symbol name, signature, first documentation
  line, line range, and a short preview of the body — enough to decide relevance
  without opening the file.
- Each result carries its lexical score, its dense score, and its fused score,
  with absence distinguishable from zero when only one retriever found it.
- The preview is bounded in length, is taken from the beginning of the chunk
  body, and never contains a partial multi-byte character.
- No result contains a whole file, and no result contains a full symbol body
  beyond the preview bound; retrieving a body is a separate, deliberate request.
- Result records are serializable to the shape the tool surface returns, and
  that shape is snapshot tested so a field cannot be renamed or reordered
  unnoticed.

### Cost

- Both retrievers are queried concurrently rather than one after the other,
  because the end-to-end budget is the sum only if they are serialized.
- Metadata for the fused candidates is fetched in one batch, not one lookup per
  result.
- End-to-end cost of a fused query — embedding, both retrievers, fusion, and
  assembly — is measured against the project direction's total budget, and the
  per-stage split is reported so a regression can be attributed.

## Constraints and non-goals

- No reranking. Phase 2, and explicitly gated on whether it improves top-1 by a
  material margin over this task's output. That gate needs this task's numbers
  as the baseline, so building a cross-encoder here would destroy the comparison.
- No learned or tuned fusion weights. Rank fusion has one constant; fitting
  weights against the evaluation set would overfit a benchmark that does not
  exist yet.
- No query classification that routes to one retriever or the other. Plausible,
  untested, and it would hide which path is carrying the results.
- No MCP tool definitions, no transport, no daemon. SIFT-010 and SIFT-011.
- No filtering by path, language, or repository. Deferred with SIFT-008's
  filtering non-goal, for the same reason.
- No deduplication of results across near-identical chunks. The store already
  deduplicates by content hash; a second similarity-based deduplication would
  silently drop distinct symbols.
- No pagination or cursors. Results are a small top-k by design.

## Acceptance criteria

### Agent-verifiable

1. Given fixed candidate lists from both retrievers, fusion produces a ranking
   matching a hand-computed expected order, including the case where a chunk
   appears in both lists.
2. A chunk present in both lists at middling rank outranks a chunk present in
   one list at similar rank, asserted on a constructed example.
3. A chunk found by only the lexical retriever appears in the final results for
   an exact-identifier query.
4. Fusion is deterministic across repeated runs, including for tied inputs.
5. When one retriever returns an error, results come from the other and the
   caller can observe that degradation.
6. Result records contain every required field, previews respect the length
   bound and never split a multi-byte character, and no record carries a body
   beyond that bound.
7. Missing scores are distinguishable from zero scores in the serialized form.
8. The serialized result shape matches a committed snapshot.
9. Both retrievers are shown to run concurrently, asserted by a test that would
   fail if they were serialized.
10. Metadata for all fused candidates is fetched in a bounded number of queries.
11. The full validation suite passes: `./ci.sh`

### Human-verifiable

1. End-to-end fused query latency over an index of at least 100,000 chunks is
   measured over 200 queries after warm-up; median and 95th percentile are
   reported with the per-stage split against the 400 ms budget.  
   Command: `cargo run --release -p retrieval --example bench_search -- <store-path> --queries 200 --stage-timings`
2. Results for a set of hand-written natural-language questions about a real
   repository are read and judged for whether the metadata alone is sufficient
   to decide relevance without opening a file.  
   Command: `cargo run --release -p retrieval --example search -- <store-path> --query "<question>"`
