# SIFT-007 implementation plan: Lexical retrieval

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/SIFT-007-lexical-retrieval-spec.md`](../specs/SIFT-007-lexical-retrieval-spec.md)  
**Depends on:** SIFT-002, SIFT-003

## Current-system context

`crates/retrieval` is empty from SIFT-001. `storage::ChunkStore` (SIFT-002)
assigns `RowId` values, holds records with `symbol`, `signature`,
`doc_first_line`, and `file`, exposes `get_many` for batched resolution, and
tracks live and dead rows; `insert_batch` and `tombstone` are the only mutating
paths, and both currently touch SQLite and the matrix only. `indexing::Chunk`
(SIFT-003) carries the `body` text, which is the text that must be searched and
which the store deliberately does not persist.

`tantivy` is named in `docs/tech-stack.md` for BM25 with an on-disk index and
near-instant open, and is not yet a workspace dependency. The gap this task
closes is that no query of any kind can be run against an index, and — the part
that constrains the design — the store persists no body text, so the lexical
index must be the thing that holds searchable text and must be updated by the
same operations that update the store.

## Interfaces produced

```rust
// crates/retrieval/src/lexical.rs
pub struct LexicalIndex { /* tantivy Index, IndexReader, prepared query parser */ }

/// One retriever's opinion. Shared shape with dense results so SIFT-009 fuses
/// without translation.
pub struct ScoredRow {
    pub row: storage::RowId,
    pub score: f32,
}

impl LexicalIndex {
    /// Creates or opens beside the ChunkStore, in the same store directory.
    pub fn open(dir: &Path) -> Result<Self, RetrievalError>;

    /// Called by the same batch that calls ChunkStore::insert_batch.
    pub fn add_batch(&mut self, docs: &[(storage::RowId, LexicalDoc)]) -> Result<(), RetrievalError>;
    /// Called by the same operation that calls ChunkStore::tombstone.
    pub fn remove(&mut self, rows: &[storage::RowId]) -> Result<(), RetrievalError>;
    /// Rebuilds row references after ChunkStore::compact renumbers them.
    pub fn renumber(&mut self, mapping: &[(storage::RowId, storage::RowId)]) -> Result<(), RetrievalError>;
    pub fn commit(&mut self) -> Result<(), RetrievalError>;

    /// Descending score, at most `limit`. Empty result for no match.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<ScoredRow>, RetrievalError>;
    pub fn num_docs(&self) -> u64;
}

/// The fields indexed for a chunk. Weights are set at query time, not here.
pub struct LexicalDoc {
    pub symbol: String,
    pub signature: String,
    pub doc_first_line: Option<String>,
    pub file: String,
    pub body: String,
}
```

```rust
// crates/retrieval/src/tokenize.rs
/// Splits identifiers so `normalizeTimestamp`, `normalize_timestamp`, and
/// `Tracker::update` are all reachable from their parts and from the whole.
pub struct CodeTokenizer { /* boundary rules; see Implementation decisions */ }

/// Emits, for `parseHTTPResponse`: the original token, then `parse`, `http`,
/// `response`. Case-folded. Order is emission order, positions are sequential.
pub fn split_identifier(token: &str) -> Vec<String>;
```

## Implementation decisions

- **The lexical index lives in the same directory as the `ChunkStore` and is
  opened alongside it.** Two directories means two things that can be moved,
  backed up, or deleted independently, and the failure mode is a search index
  describing chunks that no longer exist.

- **`RowId` is stored as a fast field on every document and returned as the
  result identity; tantivy's internal document address is never exposed.**
  Tantivy renumbers documents on merge, so an exposed address is a reference
  that silently becomes wrong.

- **Body text is stored in the lexical index and not in the `ChunkStore`.** It
  has to live exactly once. The store's job is metadata and vectors; duplicating
  bodies there would roughly double the on-disk footprint for text that only the
  lexical path reads.

- **`add_batch`, `remove`, and `renumber` are separate calls the indexer makes
  alongside the store's own, rather than the store calling into this crate.**
  The dependency runs `retrieval -> storage`; reversing it would put a search
  library behind the storage abstraction, which SIFT-002's non-goals explicitly
  rule out. The cost is that SIFT-006 must call both, which its integration
  tests assert.

- **Identifier splitting emits the original token and its parts at the same
  position.** Emitting only the parts makes an exact-identifier query rank
  every chunk containing `parse` alongside the one containing
  `parseHTTPResponse`; emitting only the whole makes a query for `parse http
  response` miss. Emitting both, with the whole token intact, lets BM25's
  inverse document frequency do the discrimination — a rare whole identifier
  outweighs its common parts automatically.

- **Splitting rules are: underscore and hyphen boundaries, lower-to-upper
  transitions, and the boundary before the last capital in a run of capitals
  followed by lowercase, so `HTTPResponse` yields `http` and `response`.**
  Digits attach to the preceding token, since `utf8` and `sha256` are single
  concepts. Namespace separators — `::`, `.`, `->` — are boundaries, so
  `Tracker::update` is reachable by either part.

- **No stemming is applied.** Stemmers are built for prose and conflate
  identifiers that mean different things — `parses` and `parsing` are fine to
  merge, but a stemmer will also merge `caching` with `cache` and `caches` in a
  way that changes ranking unpredictably across languages. The spec rules out
  prose-tuned stemming and the cost of omitting it is small on identifier-heavy
  text.

- **Fields are weighted with symbol and signature above documentation, and
  documentation above body.** A query naming a symbol should rank that symbol's
  own chunk above the chunks that merely call it. Default weights are recorded
  as named constants with their values, so SIFT-012 can measure whether they
  earn their asymmetry.

- **Query parsing is permissive: unknown syntax is treated as literal terms and
  the parser never returns a syntax error to the caller.** An agent pasting an
  error message containing quotes and colons must get results, not a parse
  failure, and the spec requires an empty result rather than an error for a
  genuine non-match.

- **Ties are broken by ascending `RowId`.** Tantivy's ordering within a score is
  segment-dependent and changes after a merge, which would make the spec's
  determinism requirement fail intermittently — the worst kind of failure to
  diagnose.

- **`commit` is explicit and called once per index batch, not per document.**
  A tantivy commit is a durable segment write; per-document commits would make
  indexing throughput the bottleneck of SIFT-006 rather than the GPU.

- **Score semantics are documented as BM25, unnormalized, comparable within one
  query and not across queries.** SIFT-009 fuses by rank precisely because of
  that second clause, and stating it here is what justifies the choice there.

## Ordered implementation

1. Create the branch `SIFT-007-lexical-retrieval`.
2. Declare `tantivy` in `[workspace.dependencies]` and inherit it in
   `crates/retrieval`; add a dependency on `crates/storage`. Confirm `./ci.sh`
   passes. Commit.
3. Write failing unit tests for `split_identifier`: `normalizeTimestamp` yields
   the original plus `normalize` and `timestamp`; `normalize_timestamp` yields
   the same parts; `parseHTTPResponse` yields `parse`, `http`, `response`;
   `Tracker::update` yields `tracker` and `update`; `sha256` stays whole;
   `read_utf8_bom` yields `read`, `utf8`, `bom`; a token with no boundary yields
   only itself. Run and confirm they fail. Implement the splitter. Confirm they
   pass. Commit.
4. Write failing tests for the tokenizer as tantivy sees it: token positions for
   `parseHTTPResponse` place the whole token and its parts at the same position,
   and the emitted stream is case-folded. Run and confirm they fail. Implement
   `CodeTokenizer` and register it. Confirm they pass. Commit.
5. Write failing tests for schema and round trip: adding three documents with
   known `RowId`s and committing yields `num_docs() == 3`, and a search for a
   term unique to the second returns exactly its `RowId`. Run and confirm they
   fail. Implement the schema with `RowId` as a fast field and the five text
   fields. Confirm they pass. Commit.
6. Write failing tests for exact-identifier ranking on a fixture corpus where
   one chunk defines `normalize_timestamp` and five others call functions
   containing `normalize`: querying `normalize_timestamp`, `normalizeTimestamp`,
   and `normalize timestamp` each returns the defining chunk at rank one. Run
   and confirm they fail. Implement the query construction and field weights.
   Confirm they pass. Commit.
7. Write a failing test for literal error strings: a chunk containing
   `"connection reset by peer"` is returned at rank one for that phrase, and for
   the same phrase surrounded by quotes and a colon. Run and confirm it fails.
   Implement permissive parsing. Confirm it passes. Commit.
8. Write a failing snapshot test pinning the result order for three multi-word
   natural-language queries over the fixture corpus. Run and confirm it fails.
   Review the order for plausibility, then accept the snapshot. Commit.
9. Write failing tests for edge behaviour: a query matching nothing returns an
   empty vector and no error; a query of only stop-like punctuation returns an
   empty vector; `limit` is respected exactly; results are in non-increasing
   score order. Run and confirm they fail. Implement. Confirm they pass. Commit.
10. Write a failing determinism test: the same query run twice, and run again
    after forcing a segment merge, returns an identical ordering including for a
    constructed pair of equal-scoring documents. Run and confirm it fails.
    Implement the `RowId` tie-break. Confirm it passes. Commit.
11. Write failing tests for removal and renumbering: after `remove`, the removed
    rows are absent from results with no separate reindex call; after
    `renumber` with a mapping, searches return the new `RowId`s and every
    surviving document is still reachable by the same query. Run and confirm
    they fail. Implement `remove` and `renumber`. Confirm they pass. Commit.
12. Write a failing integration test asserting store and index agree: build a
    small index through the same batch calls SIFT-006 will make, tombstone some
    rows in both, and assert every `RowId` returned by `search` resolves to a
    live record via `ChunkStore::get_many`. Run and confirm it fails. Fix
    whichever side is wrong. Confirm it passes. Commit.
13. Add a test asserting the documented score semantics: a committed doc comment
    states BM25, unnormalized, within-query comparability only, and a test fails
    if the scoring implementation is swapped for a normalized one. Commit.
14. Add the `bench_lexical` example: `--queries N` runs N queries after a
    warm-up reporting median and 95th percentile; `--open-only` reports index
    open time and on-disk size. Commit.
15. Human step: run `cargo run --release -p retrieval --example bench_lexical --
    <store-path> --queries 200` against an index of at least 100,000 chunks and
    report median and 95th percentile against the design document's 30 ms
    lexical budget.
16. Human step: run the same example with `--open-only` and report index open
    time and on-disk size.
17. Run the full validation suite and confirm it passes.

## Validation

- **Unit:** `split_identifier` across camel case, snake case, capital runs,
  digits, and namespace separators; tokenizer positions and case folding; limit,
  ordering, and empty-result behaviour.
- **Integration:** exact-identifier ranking across three query spellings;
  literal error-string retrieval; removal and renumbering; agreement with
  `ChunkStore` on what is live.
- **Regression:** the committed multi-word query snapshot is the locked
  reference for ranking; a change to weights or tokenization must show as a
  snapshot diff and be justified against SIFT-012's metrics once they exist.
- **Manual:** none beyond the measurements — ranking correctness is asserted by
  fixture tests rather than judged by eye at this stage; judgement of result
  quality happens in SIFT-009 and SIFT-012.
- **Measurement:** query latency over 200 queries after warm-up at an index of
  at least 100,000 chunks, median and 95th percentile against the 30 ms budget,
  three runs; index open time and on-disk size at that scale.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
./ci.sh
cargo run --release -p retrieval --example bench_lexical -- <store-path> --queries 200
cargo run --release -p retrieval --example bench_lexical -- <store-path> --open-only
```

## Handoff

Report the field weight constants chosen and their values; the identifier
splitting rules as implemented, with the outcome for `parseHTTPResponse`,
`Tracker::update`, `sha256`, and `read_utf8_bom`; confirmation that the defining
chunk ranks first for all three spellings of an identifier query and that a
quoted error string with punctuation retrieves its chunk; median and 95th
percentile query latency over 200 queries at an index of at least 100,000
chunks, three runs with individual values, against the 30 ms budget; index open
time and on-disk size at that scale; and confirmation that every `RowId`
returned by search resolved to a live record after tombstoning, and that
ordering was identical across a forced segment merge.
