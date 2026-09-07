# SIFT-002 implementation plan: Chunk store

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/SIFT-002-chunk-store-spec.md`](../specs/SIFT-002-chunk-store-spec.md)  
**Depends on:** SIFT-001

## Current-system context

SIFT-001 leaves `crates/storage` as an empty library with no dependencies, an
inherited edition, and no tests beyond the workspace-shape check at the root.
Nothing in the workspace reads or writes anything yet. The workspace dependency
table exists and is empty of storage libraries, so `rusqlite`, `memmap2`,
`half`, and `blake3` are declared here for the first time.

The design in `docs/cuda-mcp-rtx2060-plan.md` fixes the split this crate
implements: SQLite for metadata and symbol records, a memory-mapped fp16 matrix
for embeddings, with "row index == SQLite rowid". That equality is the invariant
this crate exists to own — it is stated in prose in the design document and
enforced nowhere. The gap this task closes is that no chunk can be persisted,
and the correspondence between the two halves has no code that maintains it.

## Interfaces produced

```rust
// crates/storage/src/record.rs
/// A chunk as it is stored. Mirrors the record shape in the design document.
pub struct ChunkRecord {
    pub repository: String,
    pub file: String,            // repository-relative, forward slashes on all platforms
    pub language: String,
    pub symbol: String,
    pub symbol_type: String,
    pub signature: String,
    pub doc_first_line: Option<String>,
    pub line_start: u32,         // 1-based, inclusive
    pub line_end: u32,           // 1-based, inclusive
    pub content_hash: ContentHash,
}

/// blake3 over the normalized symbol body. Excludes the file path by design.
pub struct ContentHash([u8; 32]);

/// Position of a chunk's embedding in the matrix. Assigned only by the store.
pub struct RowId(u64);
```

```rust
// crates/storage/src/matrix.rs
/// Memory-mapped fp16 matrix, one fixed-width row per chunk.
pub struct EmbeddingMatrix { /* mmap handle, header, row width */ }

/// On-disk header. Written once; read and checked on every open.
pub struct MatrixHeader {
    pub magic: [u8; 8],
    pub format_version: u32,
    pub dims: u32,               // embedding width in elements
    pub rows: u64,               // rows allocated, live and dead
    pub model_id: String,        // model identity + revision from SIFT-004 metadata
}

impl EmbeddingMatrix {
    pub fn create(path: &Path, dims: u32, model_id: &str) -> Result<Self, StoreError>;
    pub fn open(path: &Path) -> Result<Self, StoreError>;
    pub fn append(&mut self, vector: &[f16]) -> Result<RowId, StoreError>;
    pub fn row(&self, row: RowId) -> Result<&[f16], StoreError>;
    /// Whole live matrix as a contiguous slice for SIFT-008's multiplication.
    pub fn as_slice(&self) -> &[f16];
    pub fn dims(&self) -> u32;
    pub fn model_id(&self) -> &str;
}
```

```rust
// crates/storage/src/store.rs
pub struct ChunkStore { /* rusqlite connection + EmbeddingMatrix */ }

/// Counts used by SIFT-006 to decide when to compact.
pub struct StoreStats {
    pub live: u64,
    pub dead: u64,
    pub dead_fraction: f64,      // dead / (live + dead); 0.0 when empty
}

/// Result of the correspondence check. Never panics on a corrupt store.
pub enum Integrity {
    Ok { live: u64 },
    Broken { orphan_rows: Vec<RowId>, missing_rows: Vec<RowId>, duplicate_rows: Vec<RowId> },
}

impl ChunkStore {
    pub fn create(dir: &Path, dims: u32, model_id: &str) -> Result<Self, StoreError>;
    pub fn open(dir: &Path) -> Result<Self, StoreError>;

    /// Atomic batch. Returns one distinct RowId per input occurrence.
    pub fn insert_batch(&mut self, chunks: &[(ChunkRecord, Vec<f16>)]) -> Result<Vec<RowId>, StoreError>;

    pub fn get(&self, row: RowId) -> Result<Option<ChunkRecord>, StoreError>;
    /// One query for the whole set, in the order requested.
    pub fn get_many(&self, rows: &[RowId]) -> Result<Vec<Option<ChunkRecord>>, StoreError>;
    pub fn get_by_hash(&self, hash: &ContentHash) -> Result<Option<(RowId, ChunkRecord)>, StoreError>;
    pub fn rows_for_file(&self, file: &str) -> Result<Vec<RowId>, StoreError>;

    pub fn tombstone(&mut self, rows: &[RowId]) -> Result<(), StoreError>;
    pub fn stats(&self) -> Result<StoreStats, StoreError>;
    pub fn verify(&self) -> Result<Integrity, StoreError>;
    /// Rewrites both halves, renumbering live rows densely from zero.
    pub fn compact(&mut self) -> Result<CompactionReport, StoreError>;

    pub fn matrix(&self) -> &EmbeddingMatrix;
    /// Commit the index was built from; None on a fresh store. Used by SIFT-006.
    pub fn indexed_commit(&self) -> Result<Option<String>, StoreError>;
    pub fn set_indexed_commit(&mut self, commit: &str) -> Result<(), StoreError>;
}

pub enum StoreError {
    DimensionMismatch { expected: u32, got: u32 },
    ModelMismatch { expected: String, got: String },
    SchemaVersion { expected: u32, got: u32 },
    Corrupt(Integrity),
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
}
```

## Implementation decisions

- **`RowId` is a newtype the store alone constructs, and it is the SQLite
  rowid.** Exposing a bare `u64` invites a caller to compute a position from an
  index in a result vector; the first time that vector is filtered, every
  subsequent lookup is silently off by the number of elements dropped. A newtype
  with a private constructor makes that unrepresentable.

- **Metadata and matrix are written inside one SQLite transaction, with the
  matrix appended first and the transaction committed second.** A crash between
  the two leaves an appended row with no record — a dead row, which the
  correspondence check reports and compaction reclaims. The reverse order leaves
  a record pointing at a row that does not exist, which is unrecoverable
  corruption. Losing space is recoverable; losing correspondence is not.

- **SQLite runs in WAL mode with `synchronous = NORMAL`.** WAL is what lets
  SIFT-010 serve reads during an index write. `FULL` would fsync on every batch
  and dominate index time; `NORMAL` under WAL risks losing only the last
  transaction on power loss, and the spec's durability requirement is
  "openable and verifiable, losing at most the interrupted batch".

- **The matrix is appended by extending the file and remapping, growing in
  chunks of a fixed number of rows rather than one row at a time.** Remapping
  per row would dominate a 200,000-chunk index; growing in blocks amortizes it,
  at the cost of a file slightly larger than the live data, which the header's
  `rows` field makes unambiguous.

- **`as_slice` exposes the whole matrix including dead rows, and dead-row
  exclusion is SIFT-008's mask.** Filtering dead rows here would require
  materializing a compacted copy per query, defeating the memory-map. The mask
  is a bitmap the store maintains, cheap to consult per row and cheap to build.

- **The content hash is not unique among live records.** File and source
  metadata are occurrence-specific, so identical normalized content in two
  files must retain two row positions. The indexing pipeline deduplicates
  embedding computation without collapsing those metadata occurrences.

- **`insert_batch` returns one distinct `RowId` per input in input order.**
  Returning only newly created rows would force the caller to correlate
  occurrence metadata by hash, and that correlation is exactly the error this
  crate exists to prevent.

- **`get_many` binds the row set into a single query using a temporary table or
  a carray-style binding rather than issuing one statement per row.** Fusion in
  SIFT-009 resolves tens of positions per query inside a 400 ms end-to-end
  budget; per-row statements make metadata lookup a measurable fraction of it.

- **Compaction writes a new matrix file and a new metadata table beside the
  originals and swaps by rename.** Rewriting in place means a crash during
  compaction destroys the store; a rename is atomic on the platforms targeted,
  so a crash leaves the original intact.

- **`verify` returns `Integrity` rather than a boolean, and names offending
  rows.** A boolean tells an operator that something is wrong and nothing else,
  which turns every corruption into a manual investigation.

- **Schema version and matrix format version are separate integers checked on
  open.** They change for different reasons — a new metadata column does not
  invalidate embeddings — and one combined version would force a full re-embed
  for a metadata change.

- **The model identity in the matrix header is the identity string SIFT-004
  records, not a free-form label.** A store built with the primary model and
  queried with the fallback returns confident nonsense, and the header is the
  only place that can be caught.

## Ordered implementation

1. Create the branch `SIFT-002-chunk-store`.
2. Declare `rusqlite` (bundled), `memmap2`, `half`, `blake3`, `thiserror`, and
   dev-dependencies `proptest` and `tempfile` in `[workspace.dependencies]`, and
   inherit them in `crates/storage`. Confirm `./ci.sh` passes. Commit.
3. Write failing unit tests for `MatrixHeader`: creating a matrix with dims 1024
   and reading it back yields dims 1024 and the same model identity; opening a
   file with a wrong magic returns `StoreError::Corrupt`; opening one with a
   different `format_version` returns `StoreError::SchemaVersion`. Run and
   confirm they fail. Implement the header. Confirm they pass. Commit.
4. Write failing tests for `EmbeddingMatrix`: appending three vectors of width 4
   returns row ids 0, 1, 2; `row(1)` returns the second vector bit-for-bit;
   appending a vector of width 5 returns `DimensionMismatch { expected: 4,
   got: 5 }`; `as_slice().len()` equals `rows * dims`. Run and confirm they
   fail. Implement append with block growth and remap. Confirm they pass.
   Commit.
5. Write failing tests for the SQLite schema and single-chunk round trip: insert
   one record, read it back by row, by hash, and by file path, asserting every
   field equals what was written. Run and confirm they fail. Implement the
   schema, the migrations check, and the accessors. Confirm they pass. Commit.
6. Write a failing property test: for a generated set of distinct records with
   distinct hashes, every record read back by row equals the record written, and
   the row ids returned are distinct and dense from zero. Run and confirm it
   fails. Implement `insert_batch` in a transaction. Confirm it passes. Commit.
7. Write failing tests for identical occurrences: inserting a batch containing
   two records with the same content hash returns two distinct row ids and
   grows the matrix by two rows. Run and confirm they fail. Implement the
   occurrence-preserving insert path. Confirm they pass. Commit.
8. Write failing tests for `get_many`: requesting five rows returns five results
   in the order requested, with `None` for a tombstoned row; a counting hook
   asserts the number of prepared statements executed is independent of the
   number of rows requested. Run and confirm they fail. Implement the batched
   query. Confirm they pass. Commit.
9. Write failing tests for tombstoning and `stats`: after deleting 200 of 1000
   chunks, `dead` is 200, `live` is 800, and `dead_fraction` is 0.2 within
   floating-point tolerance; deleted rows return `None` from `get` and are
   absent from `rows_for_file`. Run and confirm they fail. Implement
   tombstoning and the live/dead bitmap. Confirm they pass. Commit.
10. Write failing tests for `verify`: a healthy store returns `Integrity::Ok`
    with the live count; a store with a metadata record deleted out of band
    returns `Broken` naming that row in `orphan_rows`; a store whose matrix is
    truncated returns `Broken` naming the missing rows. Run and confirm they
    fail. Implement `verify`. Confirm they pass. Commit.
11. Write failing tests for `compact`: after tombstoning 30% of 1000 chunks,
    compaction leaves exactly 700 rows, every surviving record's fields are
    unchanged, every surviving vector is bit-for-bit identical, `verify` returns
    `Ok { live: 700 }`, and previously-returned row ids for surviving chunks are
    renumbered densely. Run and confirm they fail. Implement compaction as
    write-beside-and-rename. Confirm they pass. Commit.
12. Write a failing test for atomicity: an `insert_batch` whose final write is
    forced to fail leaves the store openable, `verify` passing, and none of the
    batch's records present. Run and confirm it fails. Ensure the transaction
    boundary covers the whole batch. Confirm it passes. Commit.
13. Write failing tests for model and schema guards: opening a store and reading
    the matrix against a different model identity returns `ModelMismatch` naming
    both; opening a store whose schema version differs returns `SchemaVersion`.
    Run and confirm they fail. Implement the guards. Confirm they pass. Commit.
14. Add the `fill_and_report` example: fills a store with a given chunk count of
    synthetic records at production width, then reports on-disk size of each
    half, open time, and `verify` duration. Commit.
15. Add `scripts/kill-during-write.sh`: starts a large batch write, sends
    `SIGKILL` mid-write, then reopens the store and runs `verify`, printing the
    outcome. Commit.
16. Human step: run `cargo run --release -p storage --example fill_and_report --
    --chunks 200000` and report matrix size, database size, open time, and
    `verify` duration, comparing the matrix size against the ~400 MB the design
    document predicts.
17. Human step: run `scripts/kill-during-write.sh` and report that the store
    reopens and verifies with no manual repair.
18. Run the full validation suite and confirm it passes.

## Validation

- **Unit:** header parsing and version guards; matrix append, read, width
  rejection; record round trip by row, hash, and path; tombstone accounting;
  `verify` on healthy and on three distinct corruptions; compaction preserving
  fields and vectors.
- **Integration:** batch insert with repeated content across a generated
  corpus, followed by `verify`; batch insert interrupted by a forced failure,
  followed by reopen and `verify`.
- **Regression:** none — this task establishes the on-disk format. The format
  version constant is introduced here at 1 and becomes the reference later tasks
  must not silently change.
- **Manual:** a `SIGKILL` during a large batch write; correct means the store
  reopens and `verify` returns `Ok` without manual repair.
- **Measurement:** on-disk size of both halves at 200,000 chunks × 1024 dims
  against the predicted ~400 MB; store open time and `verify` duration at that
  size, over five runs, reporting individual values and the median.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
./ci.sh
cargo run --release -p storage --example fill_and_report -- --chunks 200000
scripts/kill-during-write.sh
```

## Handoff

Report the on-disk size of the matrix and of the database at 200,000 chunks and
how the matrix size compares against the predicted ~400 MB; store open time and
`verify` duration at that size with individual values and the median over five
runs; confirmation that `verify` detected each of the three deliberate
corruptions and named the offending rows; the measured statement count for
`get_many` at 1, 10, and 100 rows, showing it does not grow with the row count;
that compaction preserved every surviving record and vector bit-for-bit; and the
outcome of the `SIGKILL` test, including whether any records from the
interrupted batch survived.
