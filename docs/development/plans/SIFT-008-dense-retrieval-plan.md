# SIFT-008 implementation plan: Dense retrieval

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/SIFT-008-dense-retrieval-spec.md`](../specs/SIFT-008-dense-retrieval-spec.md)  
**Depends on:** SIFT-002, SIFT-005

## Current-system context

`storage::EmbeddingMatrix` (SIFT-002) memory-maps an fp16 matrix and exposes
`as_slice()` returning the whole matrix including dead rows, with `dims()` and
`model_id()` from its header; the store maintains a live/dead bitmap that
`stats()` reports but which nothing yet consumes per row. `inference::Embedder`
(SIFT-005) produces L2-normalized fp16 vectors of `dims()` width and distinguishes
`Role::Query` from `Role::Document` by metadata prefix. `retrieval::ScoredRow`
already exists from SIFT-007 and is the shared result shape.

`crates/retrieval` currently depends on `storage` and `tantivy` and has no GPU
dependency. The design document's storage section makes the bet this task must
test: 200k chunks × 1024 dims × fp16 is ~400 MB and "a single `X @ q` on the GPU
is sub-millisecond and exact", with a 10 ms budget. The gap this task closes is
that the embedding matrix has never been read for search, and the bet has never
been measured.

## Interfaces produced

```rust
// crates/retrieval/src/dense.rs
pub struct DenseIndex<'a> {
    /* &EmbeddingMatrix, live bitmap, optional GPU-resident copy */
}

impl<'a> DenseIndex<'a> {
    /// Prepared once at daemon start. `backend` decides where the matrix lives.
    pub fn prepare(matrix: &'a storage::EmbeddingMatrix, live: &LiveMask,
                   backend: DenseBackend) -> Result<Self, RetrievalError>;

    /// `query` must be L2-normalized, `dims`-wide, from the matrix's model.
    /// Descending similarity, at most `limit`, dead rows excluded.
    pub fn search(&self, query: &[f16], model_id: &str, limit: usize)
        -> Result<Vec<ScoredRow>, RetrievalError>;

    /// Rows added since prepare(); appended without recomputing the rest.
    pub fn refresh(&mut self, matrix: &'a storage::EmbeddingMatrix, live: &LiveMask)
        -> Result<(), RetrievalError>;

    pub fn resident_bytes(&self) -> u64;
}

pub enum DenseBackend {
    /// cuBLAS gemv over a device-resident copy of the matrix.
    Cuda,
    /// Same arithmetic on the host. For CI and for the naive reference.
    Cpu,
}

/// Dead-row exclusion without touching SQLite per candidate.
pub struct LiveMask { /* one bit per row, built from ChunkStore::stats path */ }

impl LiveMask {
    pub fn from_store(store: &storage::ChunkStore) -> Result<Self, RetrievalError>;
    pub fn is_live(&self, row: storage::RowId) -> bool;
}
```

```rust
// crates/retrieval/src/dense_reference.rs  (test-only)
/// Naive f32 dot product over every live row, sorted. The oracle the GPU and
/// CPU backends are both checked against.
pub fn reference_search(matrix_f32: &[f32], live: &LiveMask, dims: usize,
                        query: &[f32], limit: usize) -> Vec<ScoredRow>;
```

## Implementation decisions

- **Similarity is the dot product, not a cosine computed at query time.**
  SIFT-005 L2-normalizes every stored vector and every query, so the dot product
  *is* the cosine; recomputing norms per query would cost a full pass over the
  matrix to produce identical numbers. This is stated here because it is only
  true while normalization holds, and a test asserts stored rows are unit-norm.

- **The matrix is copied to the device once at `prepare` and reused across
  queries; `refresh` appends only new rows.** Uploading 400 MB per query is
  roughly two orders of magnitude over the 10 ms budget on this hardware. The
  cost is a second resident copy of the matrix in VRAM, which is why
  `resident_bytes` exists and is reported against the budget.

- **The multiplication is a matrix-vector product through cuBLAS, not a custom
  kernel.** The design document forbids starting with custom CUDA, and a library
  gemv on a 200k × 1024 fp16 matrix is a well-optimized memory-bound operation
  that a first custom kernel will not beat.

- **Accumulation is f32 while storage stays f16.** Summing 1024 half-precision
  products accumulates rounding error large enough to reorder near-ties, and the
  spec requires that half-precision storage not reorder the top results relative
  to a full-precision reference. cuBLAS is configured for f16 inputs with f32
  compute for exactly this.

- **Dead rows are excluded by a bitmap consulted during top-k selection, not by
  filtering the matrix or by a metadata lookup per candidate.** Filtering the
  matrix means rebuilding it on every tombstone; a metadata lookup per candidate
  turns a sub-millisecond operation into 200,000 SQLite queries.

- **Every live row participates; there is no pruning, sampling, or clustering.**
  This is the design document's central bet and the spec's strongest non-goal.
  Exhaustive is what makes the result exact, and at this corpus size it is also
  what makes it fast.

- **Top-k selection uses a bounded min-heap over the scores rather than a full
  sort.** Sorting 200,000 scores to take 50 is the dominant cost once the
  multiplication is fast, and a heap of size `limit` makes selection linear.

- **Ties are broken by ascending `RowId`, matching SIFT-007.** Two retrievers
  with different tie rules would make SIFT-009's fusion non-deterministic in
  exactly the cases where fusion matters most.

- **`model_id` and width are checked against the matrix header on every search,
  not only at prepare.** The daemon can outlive a store swap, and the spec
  requires refusal rather than confident nonsense.

- **`DenseBackend::Cpu` implements the same top-k path with the same accumulation
  precision and is not merely a debug aid.** It is what lets the whole crate be
  tested on CPU-only CI, and it is the second independent implementation the GPU
  path is checked against.

- **The GPU backend sits behind the existing `cuda` feature and the crate
  builds and tests without it.** `crates/retrieval` must stay usable by `eval`
  on machines without a GPU, per SIFT-001's structural rule.

- **`refresh` is called by the daemon after an index update rather than the
  index pushing into the search path.** SIFT-010 owns when a search sees new
  rows, and a push would let a query observe a partially uploaded batch.

## Ordered implementation

1. Create the branch `SIFT-008-dense-retrieval`.
2. Add `cudarc` as an optional dependency of `crates/retrieval` under the `cuda`
   feature, and a dependency on `crates/inference` with default features for the
   `Embedder` trait and `MockEmbedder`. Confirm `cargo build --workspace`
   succeeds without the feature. Commit.
3. Write failing unit tests for `LiveMask`: a store with 100 chunks and 10
   tombstoned reports `is_live` false for exactly those 10; the mask's size
   matches the matrix's row count including dead rows. Run and confirm they
   fail. Implement `LiveMask::from_store`. Confirm they pass. Commit.
4. Write `reference_search` and failing tests for it on a hand-built 5×3 matrix
   with hand-computed dot products, asserting the exact returned order and
   scores, and asserting a masked row is absent. Run and confirm they fail.
   Implement the reference. Confirm they pass. Commit.
5. Write failing tests for the CPU backend against the reference: on a randomly
   generated 1000×64 normalized matrix with 50 random queries, the returned
   row order and scores match `reference_search` exactly. Run and confirm they
   fail. Implement `DenseBackend::Cpu` with heap-based top-k. Confirm they pass.
   Commit.
6. Write a failing test using `MockEmbedder`: build a store from fixture texts,
   construct a query with `query_matching` for a known text, and assert its row
   is returned at rank one. Run and confirm it fails. Wire `prepare` from a real
   `ChunkStore`. Confirm it passes. Commit.
7. Write a failing precision test: over 200 trials of a randomly generated
   normalized matrix, assert the top-10 row order from the f16 matrix with f32
   accumulation equals the top-10 from an f32 reference; report any trial where
   it differs. Run and confirm it fails. Confirm the accumulation precision.
   Confirm it passes. Commit.
8. Write failing tests for guards: a query of width `dims + 1` returns
   `DimensionMismatch` naming both widths; a query tagged with a different
   `model_id` returns `ModelMismatch` naming both. Run and confirm they fail.
   Implement per-search checks. Confirm they pass. Commit.
9. Write failing tests for exclusion and limits: after tombstoning rows that
   would otherwise rank in the top 5, they are absent and the next-best rows
   take their places; `limit` is respected exactly; results are non-increasing;
   a limit larger than the live count returns the live count. Run and confirm
   they fail. Implement. Confirm they pass. Commit.
10. Write a failing test asserting no per-candidate metadata lookup occurs
    during search, using a counting wrapper around the store that fails the test
    if it is queried at all during `search`. Run and confirm it fails. Confirm
    exclusion goes through `LiveMask` only. Confirm it passes. Commit.
11. Write failing determinism and normalization tests: the same query run twice
    returns an identical ordering including for constructed equal scores; every
    row in a store built through `MockEmbedder` has unit norm within tolerance,
    so the dot-product-as-cosine decision holds. Run and confirm they fail.
    Implement the `RowId` tie-break. Confirm they pass. Commit.
12. Write a failing test for `refresh`: appending 100 rows after `prepare` makes
    them searchable, leaves existing rows' scores unchanged, and does not
    re-upload the whole matrix, asserted by an upload-byte counter. Run and
    confirm it fails. Implement incremental append. Confirm it passes. Commit.
13. Implement `DenseBackend::Cuda` behind the `cuda` feature: device-resident
    matrix, cuBLAS gemv with f16 input and f32 compute, device-side or
    host-side top-k as measurement dictates. Add an `#[ignore]`d test asserting
    the CUDA backend's results equal the CPU backend's exactly on a fixed
    fixture. Confirm the non-feature build still passes. Commit.
14. Add the `bench_dense` example: `--sizes` builds or loads matrices at the
    given chunk counts, `--queries N` measures per-query latency after warm-up
    reporting median and 95th percentile per size, and `--report-vram` reports
    `resident_bytes` and peak GPU memory with the embedding model also loaded.
    Commit.
15. Human step: run `cargo run --release -p retrieval --example bench_dense --
    --sizes 10000,50000,200000 --queries 200` and report median and 95th
    percentile per size against the 10 ms budget.
16. Human step: run `cargo run --release -p retrieval --example bench_dense --
    --sizes 200000 --report-vram` with a desktop session attached and the
    embedding model resident, and report peak GPU memory against the ~5.0 GB
    budget.
17. Human step: judge the exhaustive-search bet against the two measurements
    above. If 200,000 chunks does not clear the budget, record the measured
    figures as the evidence and report it — do not add an approximate index in
    this task.
18. Run the full validation suite and confirm it passes.

## Validation

- **Unit:** `LiveMask` construction; `reference_search` against hand-computed
  values; guard errors for width and model mismatch; limit, ordering, and
  tie-breaking.
- **Integration:** CPU backend against the naive reference across 50 random
  queries; `MockEmbedder` known-nearest-neighbour retrieval; tombstone exclusion
  end to end; `refresh` after append; CUDA backend against the CPU backend on a
  fixed fixture, `#[ignore]`d.
- **Regression:** the CPU backend is the locked reference for the CUDA backend;
  the two must agree exactly on the fixed fixture, and any divergence is a
  defect in the GPU path rather than a tolerance to widen.
- **Manual:** none beyond the measurements and the bet judgement in step 17.
- **Measurement:** per-query latency at roughly 10,000, 50,000, and 200,000
  chunks over 200 queries after warm-up, median and 95th percentile at each
  size, against the 10 ms budget; `resident_bytes` and peak GPU memory at the
  largest size with the embedding model also resident.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
./ci.sh
cargo test -p retrieval --release --features cuda -- --ignored cuda_matches_cpu
cargo run --release -p retrieval --example bench_dense -- --sizes 10000,50000,200000 --queries 200
cargo run --release -p retrieval --example bench_dense -- --sizes 200000 --report-vram
```

## Handoff

Report median and 95th percentile query latency at each of the three corpus
sizes over 200 queries after warm-up, with individual run values, against the
10 ms budget; how latency scaled with corpus size and whether the scaling is
consistent with a memory-bound multiplication; `resident_bytes` at 200,000
chunks and peak GPU memory with the embedding model also loaded, against the
~5.0 GB budget; the number of precision trials run and how many showed a top-10
reordering between f16 storage and the f32 reference; confirmation that the CUDA
backend matched the CPU backend exactly on the fixed fixture; confirmation that
no metadata lookup occurs per candidate, citing the counting-wrapper test; and
an explicit verdict on the exhaustive-search bet — cleared, or the measured
figures that show it did not.
