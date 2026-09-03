# SIFT-002: Chunk store

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Project direction:** [`../../cuda-mcp-rtx2060-plan.md`](../../cuda-mcp-rtx2060-plan.md), [`../../tech-stack.md`](../../tech-stack.md)  
**Depends on:** SIFT-001  
**Implementation plan:** [`../plans/SIFT-002-chunk-store-plan.md`](../plans/SIFT-002-chunk-store-plan.md)

## Purpose

Chunk metadata and chunk embeddings are two different shapes of data — small
structured records that are queried, and a large dense numeric matrix that is
multiplied — and the design deliberately keeps them in two different stores. The
danger in that split is that nothing enforces their correspondence: a row of the
matrix that no longer describes the record claiming it produces confidently
wrong search results with no error anywhere. This task builds both stores and
makes their correspondence an invariant the store itself maintains and can
verify. Every task that indexes, searches, or evaluates reads through it.

## Requirements

### Correspondence

- A chunk's metadata record and its embedding are allocated together, and the
  store is the only thing that assigns the position of an embedding.
- The position of an embedding is never derived by a caller, never inferred from
  ordering, and never reused while a live record refers to it.
- The store can verify, on demand, that every live record refers to a distinct
  existing position and that no live position is unreferenced, and reports the
  specific offending records when it cannot.
- Opening a store whose two halves disagree fails loudly rather than proceeding
  with whichever half is smaller.

### Records and lookup

- A stored chunk carries the repository, file path, language, symbol name and
  kind, signature, first line of documentation, start and end line numbers, and
  a content hash — the fields a caller needs to triage a result without opening
  the file.
- Chunks are retrievable by position, by content hash, and by file path, and
  every file's chunks are retrievable together, because incremental re-indexing
  works a file at a time.
- The content hash is unique among live records: two chunks with identical
  normalized bodies share one embedding rather than occupying two positions.
- Retrieving a batch of chunks by a set of positions costs one round trip, not
  one per position, because ranking returns tens of positions at once.

### Embedding matrix

- Embeddings are stored at half precision, contiguously, one fixed-width row per
  chunk, in a form readable as a matrix without deserialization or copying.
- The matrix records its own dimensionality and the identity of the model that
  produced it, and refuses to accept a vector of a different width or to be read
  against a query from a different model.
- Appending an embedding does not rewrite or move existing rows.
- Reading the matrix does not require it to fit in memory as a copy.

### Deletion and compaction

- Removing a chunk marks its position dead rather than moving live rows, so
  positions held by an in-flight query stay valid.
- Dead positions are excluded from search results, and the store reports how
  many it holds and what fraction of the total that is.
- Compaction reclaims dead positions, renumbers the live ones consistently
  across both halves, and leaves the store verifiable by the check above.
- Compaction is explicit. It never happens as a side effect of a write, because
  a write path that sometimes rewrites the entire matrix has no usable latency
  characteristic.

### Durability

- A batch of chunks either lands entirely or not at all; an interrupted index
  run leaves the store openable and verifiable, not half-written.
- The store survives a process kill mid-write without requiring manual repair,
  losing at most the interrupted batch.

## Constraints and non-goals

- No search. No ranking, no similarity, no text matching. The store returns
  records and exposes the matrix; SIFT-007 and SIFT-008 do the searching. The
  temptation to "add a quick cosine helper here" is ruled out — it would put the
  hot numeric path behind the storage abstraction where it cannot be optimized.
- No embedding generation. The store accepts vectors it is given.
- No chunking. The store accepts records it is given; SIFT-003 produces them.
- No approximate-nearest-neighbour index, no vector database, no clustering
  structure. The project direction rules these out below roughly two million
  chunks and this task does not reopen that.
- No automatic compaction scheduling or background maintenance thread. The
  policy for when to compact belongs to the indexing pipeline in SIFT-006.
- No cross-process concurrent writers. A single writer at a time is assumed;
  concurrent readers are not.
- No schema migration framework. The schema is versioned and a mismatch is an
  error; migrating an existing store is a later task if it is ever needed.

## Acceptance criteria

### Agent-verifiable

1. Allocating chunks and reading them back by position, hash, and file path
   returns identical records, verified against property-based generated input.
2. Inserting a chunk whose content hash already exists reuses the existing
   position and does not grow the matrix.
3. Writing a vector of the wrong width, or reading against a model identifier
   that does not match the one recorded, fails with a distinguishable error.
4. The verification check passes on a healthy store and, on a store deliberately
   corrupted by removing a metadata record without its embedding, fails and
   names the offending position.
5. After deleting a known fraction of chunks, the reported dead fraction matches
   the fraction deleted, and dead positions are absent from enumerated results.
6. Compaction reduces the matrix to exactly the live count, preserves every live
   chunk's fields and vector bit-for-bit, and leaves the verification passing.
7. A batch write interrupted by a simulated failure leaves the store openable,
   verifiable, and containing none of the interrupted batch.
8. Reading a set of positions issues a bounded number of queries independent of
   the size of the set.
9. The full validation suite passes: `./ci.sh`

### Human-verifiable

1. A store holding 200,000 chunks at the production embedding width is built and
   its on-disk size is reported, confirming the matrix is close to the ~400 MB
   the project direction predicts and that opening it is near-instant.  
   Command: `cargo run --release -p storage --example fill_and_report -- --chunks 200000`
2. The process is killed with `SIGKILL` during a large batch write and the store
   is confirmed to reopen and verify without manual repair.  
   Command: `scripts/kill-during-write.sh`
