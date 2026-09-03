# SIFT-009 implementation plan: Fusion and result assembly

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/SIFT-009-fusion-and-results-spec.md`](../specs/SIFT-009-fusion-and-results-spec.md)  
**Depends on:** SIFT-007, SIFT-008

## Current-system context

Both retrievers return `Vec<ScoredRow>` — `LexicalIndex::search` (SIFT-007) with
unnormalized BM25 scores documented as comparable only within a query, and
`DenseIndex::search` (SIFT-008) with dot-product similarities in a fixed range,
both descending and both breaking ties by ascending `RowId`. `ChunkStore::get_many`
(SIFT-002) resolves a set of `RowId`s in one query and returns records carrying
`file`, `symbol`, `signature`, `doc_first_line`, `line_start`, and `line_end`.
The chunk body lives only in the lexical index (SIFT-007's decision), which is
where the preview must come from.

Nothing composes the two retrievers, and no crate yet produces the response
shape the design document specifies under *Response shape: metadata-first*. The
gap this task closes is that there is no single "search" operation, and no
result record an agent could triage without opening a file.

## Interfaces produced

```rust
// crates/retrieval/src/fusion.rs
/// Reciprocal rank fusion. `k` damps the influence of top ranks; see decisions.
pub struct FusionConfig {
    pub lexical_depth: usize,   // candidates taken from the lexical retriever
    pub dense_depth: usize,     // candidates taken from the dense retriever
    pub rrf_k: f32,
}

/// One retriever's contribution to a fused row. None means "did not return it",
/// which is distinct from a score of zero.
pub struct Contribution {
    pub rank: Option<u32>,      // 1-based within that retriever's list
    pub score: Option<f32>,
}

pub struct FusedRow {
    pub row: storage::RowId,
    pub lexical: Contribution,
    pub dense: Contribution,
    pub fused_score: f32,
}

pub fn fuse(lexical: &[ScoredRow], dense: &[ScoredRow], config: &FusionConfig)
    -> Vec<FusedRow>;
```

```rust
// crates/retrieval/src/search.rs
pub struct Searcher<'a> { /* &LexicalIndex, &DenseIndex, &ChunkStore, &dyn Embedder */ }

/// Which retrievers ran and which failed. Degradation is data, not a log line.
pub struct SearchDiagnostics {
    pub lexical_ok: bool,
    pub dense_ok: bool,
    pub lexical_error: Option<String>,
    pub dense_error: Option<String>,
    pub stage_millis: StageTimings,
}

pub struct StageTimings {
    pub embed: u64,
    pub lexical: u64,
    pub dense: u64,       // wall time of the concurrent pair is max, not sum
    pub fuse: u64,
    pub assemble: u64,
    pub total: u64,
}

pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub diagnostics: SearchDiagnostics,
}

impl<'a> Searcher<'a> {
    pub fn search(&self, query: &str, top_k: usize, config: &FusionConfig)
        -> Result<SearchResponse, RetrievalError>;
    /// find_similar_code: embeds `code` as a document, skips the lexical path's
    /// query parsing, and otherwise shares the pipeline.
    pub fn search_similar(&self, code: &str, top_k: usize, config: &FusionConfig)
        -> Result<SearchResponse, RetrievalError>;
}
```

```rust
// crates/retrieval/src/result.rs
/// The serialized shape returned to the agent. Field names and order are the
/// locked snapshot; see Implementation decisions.
pub struct SearchResult {
    pub file: String,
    pub symbol: String,
    pub signature: String,
    pub doc: Option<String>,
    pub preview: String,           // at most PREVIEW_MAX_BYTES, char-boundary safe
    pub lines: [u32; 2],           // [line_start, line_end], 1-based inclusive
    pub lexical_score: Option<f32>,
    pub dense_score: Option<f32>,
    pub fused_score: f32,
}

/// Preview ceiling in bytes. Value and rationale in Implementation decisions.
pub const PREVIEW_MAX_BYTES: usize = 320;
```

## Implementation decisions

- **Fusion is reciprocal rank fusion over rank positions, and raw scores are
  carried through for display only.** SIFT-007 documents BM25 as comparable only
  within a query; any weighted sum of a BM25 score and a dot product encodes an
  exchange rate that is wrong for some queries and cannot be right for all. Rank
  is the only quantity both lists share.

- **`rrf_k` defaults to 60.** That is the value from the original rank-fusion
  literature and the one most reported results assume; choosing a different
  default without evidence would make this system's numbers incomparable to
  every published baseline. SIFT-012 can revisit it with measurements.

- **`lexical_depth` and `dense_depth` default to 50 each.** The design document
  specifies top-50 from each path feeding a union of roughly 75 unique
  candidates, which is also the candidate-set size Phase 2's reranker is
  budgeted for at 300 ms. Setting them lower now would shrink the pool the
  reranker gate is evaluated against.

- **The fused list is a union, not an intersection, and a row present in one
  list scores as if its rank in the other were infinite.** The lexical path
  exists precisely to catch exact identifiers the dense path misses; an
  intersection would discard exactly those.

- **`Contribution` distinguishes `None` from `Some(0.0)`.** A missing score
  serialized as zero tells the agent the retriever ranked it worst, when in fact
  the retriever never saw it — and the same ambiguity would corrupt SIFT-012's
  ablation analysis.

- **Ties in `fused_score` break by ascending `RowId`, matching both retrievers.**
  Three components with three tie rules produce orderings that change between
  runs, and the spec requires determinism.

- **The two retrievers are dispatched concurrently and `StageTimings::total`
  reflects the wall time of the pair, not the sum.** The end-to-end budget is
  400 ms and the two paths together are budgeted at 40 ms only if they overlap.
  The concurrency is asserted by a test rather than assumed.

- **The query is embedded once, before dispatch, and both paths receive what
  they need from that single call.** Embedding is on the critical path at
  roughly 15 ms; doing it inside the dense branch would serialize it behind
  nothing but would also make `StageTimings::embed` unattributable.

- **A retriever that errors is recorded in `SearchDiagnostics` and the other's
  results are returned; only both failing is an error.** An agent that receives
  an error stops calling the tool, and a degraded result is far better than
  none — but a silent degradation would hide a broken GPU path indefinitely,
  which is why it is a field rather than a log line.

- **Metadata for the whole fused candidate set is fetched with one
  `get_many`.** Per-result lookups are tens of SQLite round trips inside a
  400 ms budget, and SIFT-002 added `get_many` for this call site.

- **The preview comes from the lexical index's stored body, truncated to
  `PREVIEW_MAX_BYTES` at a character boundary, taken from the start of the
  chunk body.** The body lives only there, so re-reading the file would add a
  filesystem round trip per result and would return the *current* file rather
  than the indexed revision. 320 bytes is roughly the three-to-four lines the
  design document's example preview shows — enough to recognize the code,
  short enough that ten results stay small.

- **`PREVIEW_MAX_BYTES` truncates on a UTF-8 character boundary and appends
  nothing.** An ellipsis inside a code preview reads as part of the code.

- **The serialized field names and order match the design document's example
  response exactly, with `lines` as a two-element array.** That example is what
  the tool descriptions in SIFT-011 will quote, and a mismatch between the
  documented shape and the returned shape is the kind of thing an agent silently
  mishandles.

- **`search_similar` reuses the pipeline but embeds its input with
  `Role::Document`.** The input is a code snippet, not a natural-language query;
  applying the query instruction prefix would place it in the wrong region of
  the embedding space from every indexed chunk.

## Ordered implementation

1. Create the branch `SIFT-009-fusion-and-results`.
2. Write failing unit tests for `fuse` on hand-built lists: with `rrf_k = 60`,
   a row at lexical rank 1 and dense rank 3 scores `1/61 + 1/63`, asserted to
   six decimal places; a row present only in the lexical list scores `1/61`
   alone and carries `dense: Contribution { rank: None, score: None }`; the
   returned order matches a hand-computed ranking. Run and confirm they fail.
   Implement `fuse`. Confirm they pass. Commit.
3. Write a failing test for the union-beats-intersection property: a row at rank
   5 in both lists outranks a row at rank 2 in only one, with the arithmetic
   stated in the test. Run and confirm it fails. Confirm `fuse` satisfies it.
   Commit.
4. Write failing tests for determinism and ties: two rows with identical
   contributions order by ascending `RowId`; running `fuse` twice on the same
   input yields identical output. Run and confirm they fail. Implement the
   tie-break. Confirm they pass. Commit.
5. Write failing tests for preview construction: a body longer than
   `PREVIEW_MAX_BYTES` truncates to at most that many bytes; a body whose byte
   320 falls inside a multi-byte character truncates shorter, at the boundary,
   and the result is valid UTF-8; a short body is returned whole; no ellipsis is
   appended. Run and confirm they fail. Implement the truncation. Confirm they
   pass. Commit.
6. Write a failing serialization snapshot test: a constructed `SearchResult` with
   both scores present, and one with `lexical_score` absent, serialize to a
   committed JSON snapshot with the field names and order from the design
   document's example, and with the absent score distinguishable from zero. Run
   and confirm it fails. Implement `SearchResult` and its serialization. Confirm
   the snapshot matches after review against the design document. Commit.
7. Write a failing integration test for `search` using `MockEmbedder` and a
   fixture store built through both indexes: a query whose answer is known
   returns it at rank one, every result carries all required fields, and
   `diagnostics.lexical_ok` and `dense_ok` are both true. Run and confirm it
   fails. Implement `Searcher::search` sequentially first. Confirm it passes.
   Commit.
8. Write a failing test asserting concurrency: instrument both retrievers to
   record entry and exit timestamps and assert their intervals overlap, and
   assert `stage_millis.total` is less than the sum of the two retrievers'
   individual times when each is made to sleep. Run and confirm it fails.
   Dispatch the two concurrently. Confirm it passes. Commit.
9. Write a failing test asserting one `get_many` call: wrap the store in a
   counting decorator and assert exactly one metadata call per search regardless
   of candidate count. Run and confirm it fails. Batch the resolution. Confirm
   it passes. Commit.
10. Write failing tests for degradation: with the dense retriever forced to
    error, results come from the lexical path, `dense_ok` is false,
    `dense_error` is populated, and the call returns `Ok`; the same with the
    retrievers swapped; with both failing, the call returns `Err`. Run and
    confirm they fail. Implement the degradation path. Confirm they pass.
    Commit.
11. Write a failing test for exact-identifier recovery through fusion: a query
    for an identifier that the mock dense path ranks poorly still returns the
    defining chunk in the final top 5, via the lexical contribution. Run and
    confirm it fails. Confirm the union semantics deliver it. Confirm it passes.
    Commit.
12. Write a failing test for `search_similar`: a code snippet copied from a
    fixture chunk returns that chunk at rank one, and a test asserts the input
    was embedded with `Role::Document` rather than `Role::Query`. Run and
    confirm they fail. Implement `search_similar`. Confirm they pass. Commit.
13. Write a failing test asserting no result carries a body beyond the preview
    bound and no result contains a whole file, over a fixture whose files are
    large. Run and confirm it fails. Confirm the assembly path. Confirm it
    passes. Commit.
14. Add the `search` example printing results as JSON for a given query, and the
    `bench_search` example with `--queries N --stage-timings` reporting median
    and 95th percentile total latency plus the per-stage split. Commit.
15. Human step: run `cargo run --release -p retrieval --example bench_search --
    <store-path> --queries 200 --stage-timings` against an index of at least
    100,000 chunks and report median and 95th percentile total against the
    400 ms budget with the per-stage split.
16. Human step: run `cargo run --release -p retrieval --example search --
    <store-path> --query "<question>"` for a set of hand-written natural-language
    questions about a real repository, read the results, and judge whether the
    metadata alone suffices to decide relevance without opening a file.
17. Run the full validation suite and confirm it passes.

## Validation

- **Unit:** `fuse` arithmetic to six decimal places on hand-built lists;
  union-beats-intersection; determinism and tie-breaking; preview truncation
  including a multi-byte boundary.
- **Integration:** end-to-end search through both indexes with `MockEmbedder`;
  concurrency of the two retrievers; single-call metadata resolution;
  degradation with each retriever failing and with both; exact-identifier
  recovery through the lexical contribution; `search_similar` role selection.
- **Regression:** the committed serialization snapshot is the locked reference
  for the response shape; SIFT-011's tool descriptions quote it, so a field
  rename must be a deliberate, reviewed change.
- **Manual:** reading results for hand-written questions about a real
  repository; correct means the file, symbol, signature, doc line, and preview
  together are enough to decide relevance without opening the file.
- **Measurement:** end-to-end latency over 200 queries after warm-up at an index
  of at least 100,000 chunks, median and 95th percentile with the per-stage
  split, three runs, against the 400 ms budget and the design document's
  per-stage figures of 15 ms embed, 30 ms lexical, and 10 ms dense.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
./ci.sh
cargo run --release -p retrieval --example bench_search -- <store-path> --queries 200 --stage-timings
cargo run --release -p retrieval --example search -- <store-path> --query "where are decoder timestamps clamped"
```

## Handoff

Report the fusion constants used — `rrf_k`, `lexical_depth`, `dense_depth` — and
the resulting mean size of the candidate union across the benchmark queries,
against the design document's expected ~75; median and 95th percentile
end-to-end latency over 200 queries with the per-stage split, individual values
across three runs, against the 400 ms budget and the per-stage figures; evidence
that the two retrievers ran concurrently, citing the overlap test; confirmation
that metadata resolution is one call per search regardless of candidate count;
the behaviour observed with each retriever forced to fail; the value of
`PREVIEW_MAX_BYTES` and confirmation that truncation is character-boundary safe;
and a judgement, from reading results for the hand-written questions, on whether
the returned metadata is sufficient to triage without opening a file — this is
the baseline the Phase 2 reranker gate will be measured against.
