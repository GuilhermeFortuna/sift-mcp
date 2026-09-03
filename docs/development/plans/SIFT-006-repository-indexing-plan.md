# SIFT-006 implementation plan: Repository indexing

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/SIFT-006-repository-indexing-spec.md`](../specs/SIFT-006-repository-indexing-spec.md)  
**Depends on:** SIFT-002, SIFT-003, SIFT-005

## Current-system context

Three pieces exist and nothing joins them. `storage::ChunkStore` (SIFT-002)
offers `insert_batch` returning one `RowId` per input with content-hash reuse
already built in, `rows_for_file`, `tombstone`, `stats` with `dead_fraction`,
`compact`, `verify`, and — added for this task and so far unused —
`indexed_commit` and `set_indexed_commit`. `indexing::Chunker` (SIFT-003)
produces `Chunk { record, body, fragment }` per file and `Exclusions` decides
skips by path before opening and by head bytes after. `inference::Embedder`
(SIFT-005) embeds a slice of texts with a `Role`, splitting at its own batch
limit and reporting truncation.

`gix` is not yet a workspace dependency and nothing in the repository reads git.
The design document's *Caching and incremental indexing* section specifies the
diff-driven update this task implements: re-parse touched files, reuse unchanged
hashes, append new ones, tombstone vanished ones, and compact above roughly 20%
tombstones. The gap this task closes is that no repository has ever been
indexed, and the second index has no reason to be cheaper than the first.

## Interfaces produced

```rust
// crates/indexing/src/pipeline.rs
pub struct Indexer<'a> {
    /* ChunkStore, Chunker, &dyn Embedder, Exclusions, IndexConfig */
}

pub struct IndexConfig {
    pub embed_batch: usize,          // texts per Embedder::embed call
    pub parse_threads: usize,        // 0 = available parallelism
    pub compact_threshold: f64,      // dead_fraction above which compact() runs
    pub dirty_worktree: DirtyPolicy,
}

/// What to do when the worktree differs from the indexed commit.
pub enum DirtyPolicy { IndexWorktree, IndexCommitOnly, Refuse }

/// Reported for every run, full or incremental. Counters reconcile with
/// ChunkStore::stats before and after; see Implementation decisions.
pub struct IndexReport {
    pub commit: String,
    pub files_seen: u64,
    pub files_indexed: u64,
    pub files_excluded: u64,
    pub files_unsupported: u64,
    pub files_unparsed: u64,
    pub chunks_added: u64,
    pub chunks_reused: u64,          // hash already live; no embedding computed
    pub chunks_removed: u64,
    pub embeddings_computed: u64,
    pub chunks_truncated: u64,
    pub parse_millis: u64,
    pub embed_millis: u64,
    pub store_millis: u64,
    pub wall_millis: u64,
    pub compacted: Option<CompactionReport>,
}

impl<'a> Indexer<'a> {
    pub fn open(store: ChunkStore, embedder: &'a dyn Embedder, repo: &Path,
                config: IndexConfig) -> Result<Self, IndexError>;

    /// Full index. Resumable: files already present at their current hash are
    /// skipped, so a re-run after an interruption completes rather than restarts.
    pub fn index_all(&mut self, progress: &mut dyn Progress) -> Result<IndexReport, IndexError>;

    /// Diff-driven. Uses ChunkStore::indexed_commit as the base.
    pub fn update(&mut self, progress: &mut dyn Progress) -> Result<IndexReport, IndexError>;
}

/// Progress is a trait so the daemon can stream it and the CLI can print it.
pub trait Progress {
    fn phase(&mut self, phase: Phase, done: u64, total: Option<u64>);
}
pub enum Phase { Walking, Parsing, Embedding, Storing, Compacting }
```

```rust
// crates/indexing/src/git.rs
/// One entry of `git diff --name-status` between two commits.
pub enum FileChange {
    Added(String),
    Modified(String),
    Deleted(String),
    Renamed { from: String, to: String },
}

pub struct RepoGit { /* gix repository handle */ }

impl RepoGit {
    pub fn open(root: &Path) -> Result<Self, IndexError>;
    pub fn head_commit(&self) -> Result<String, IndexError>;
    pub fn changes_since(&self, base: &str) -> Result<Vec<FileChange>, IndexError>;
    pub fn is_dirty(&self) -> Result<bool, IndexError>;
}
```

## Implementation decisions

- **A file is the unit of update, and a changed file is re-parsed whole and
  reconciled against `rows_for_file`.** Reconciling per file means the pipeline
  never has to know which symbol inside a file changed — the hash comparison
  answers that — and re-parsing a file is microseconds against the milliseconds
  an embedding costs.

- **Reconciliation is a set difference on content hashes: hashes present on disk
  but not in the store are embedded and inserted; hashes in the store for that
  file but not on disk are tombstoned; hashes in both are left untouched.** Any
  scheme that compares by symbol name instead fails on a rename-plus-move, which
  is the case the path-free hash from SIFT-003 exists to make free.

- **A rename is handled as "re-key the file path on existing rows", not as a
  delete followed by an add.** The bodies are unchanged, so their hashes are
  unchanged; treating it as delete-and-add would tombstone every row and
  re-insert it, which the content-hash reuse would then partly undo, leaving the
  store churned for nothing.

- **`chunks_reused` counts hashes that resolved to an existing live row, and
  `embeddings_computed` counts vectors actually produced.** The distinction is
  the whole point of the incremental design, and a single "chunks processed"
  counter would make the acceptance criterion "re-embeds exactly one chunk"
  unverifiable.

- **Parsing runs on a thread pool over files; embedding runs on a single
  consumer draining a bounded channel.** Parsing is CPU-bound and scales with
  cores; embedding is GPU-bound and one device serializes anyway. A bounded
  channel gives backpressure, so a fast parser cannot accumulate the whole
  repository's bodies in memory ahead of a slower GPU.

- **Embedding batches are assembled to `IndexConfig::embed_batch` and passed as
  `Role::Document`.** Queries carry the instruction prefix and documents do not;
  indexing with `Role::Query` would misalign every stored vector against every
  future query, degrading results with nothing failing.

- **Store writes happen in batches matching the embedding batch, inside
  `insert_batch`'s transaction.** SIFT-002 makes a batch atomic, so the largest
  unit lost to an interruption is one batch, and the resumability requirement is
  satisfied by the store's own guarantee rather than by a new checkpoint file.

- **`index_all` is resumable by checking, per file, whether the store already
  holds exactly that file's current hash set, and skipping the file when it
  does.** This costs one indexed lookup per file on a resume and requires no
  progress journal, which would be a second thing to keep consistent with the
  store.

- **`set_indexed_commit` is called once, after the final batch commits and after
  any compaction.** Advancing it earlier means an interrupted run records a
  commit it did not finish indexing, and the next `update` would diff from the
  wrong base and never notice the gap.

- **`DirtyPolicy` defaults to `IndexWorktree`, and when the worktree is dirty
  the recorded commit is suffixed with a dirty marker.** Indexing what is on
  disk is what a developer expects; recording a bare commit hash for an index
  that includes uncommitted work would make the next diff silently wrong. The
  marker makes the next `update` fall back to a full reconcile of the dirty
  files.

- **Compaction is checked once at the end of a run against
  `compact_threshold`, defaulting to 0.20 per the design document's "compact
  when tombstones exceed ~20%".** Checking mid-run would rewrite the matrix
  while rows are being appended; checking never would let the matrix grow
  without bound across many small updates.

- **A file that fails to parse is counted and leaves its existing rows
  untouched.** Tombstoning them would silently delete a symbol from the index
  because of a transient syntax error in an editor buffer, and the agent would
  get "not found" for code that exists.

- **`IndexReport` counters are asserted to reconcile:
  `live_after == live_before + chunks_added - chunks_removed`.** Counters that
  are merely reported drift from reality; an assertion in the test suite makes a
  bookkeeping bug fail loudly instead of producing a plausible summary.

## Ordered implementation

1. Create the branch `SIFT-006-repository-indexing`.
2. Declare `gix`, `rayon`, and `crossbeam-channel` in `[workspace.dependencies]`
   and inherit them in `crates/indexing`; add dependencies on `crates/storage`
   and `crates/inference` with default features. Confirm `./ci.sh` passes.
   Commit.
3. Add a test-support helper that builds a temporary git repository from a
   sequence of described commits, so every git test constructs its own history.
   Commit.
4. Write failing tests for `RepoGit`: `head_commit` returns the current hash;
   `changes_since` on a commit that added one file returns one `Added`;
   modifying returns `Modified`; deleting returns `Deleted`; a rename with
   unchanged content returns `Renamed { from, to }`; `is_dirty` is false on a
   clean tree and true after an uncommitted edit. Run and confirm they fail.
   Implement `RepoGit`. Confirm they pass. Commit.
5. Write a failing integration test for `index_all` over a fixture repository
   with a known symbol set, using `MockEmbedder`: the report's `files_indexed`,
   `chunks_added`, and `embeddings_computed` match expected values,
   `ChunkStore::verify` returns `Ok`, and every expected symbol is retrievable
   by `rows_for_file`. Run and confirm it fails. Implement the walk, the parse
   pool, the embed consumer, and batched stores. Confirm it passes. Commit.
6. Write a failing test asserting excluded paths are never opened during the
   walk, reusing SIFT-003's unreadable-sentinel technique, and that
   `files_excluded` and `files_unsupported` are counted separately. Run and
   confirm it fails. Wire `Exclusions` into the walk before `File::open`.
   Confirm it passes. Commit.
7. Write a failing test asserting the counter reconciliation identity holds
   after `index_all` on the fixture. Run and confirm it fails. Implement the
   assertion inside `IndexReport` construction. Confirm it passes. Commit.
8. Write a failing test for no-op re-index: running `index_all` twice on an
   unchanged repository leaves `embeddings_computed` at 0, `chunks_added` at 0,
   `chunks_removed` at 0, and the store's live count unchanged. Run and confirm
   it fails. Implement the per-file hash-set skip. Confirm it passes. Commit.
9. Write a failing test for `update` on a body edit: in a repository of at least
   twenty files, a commit editing one function's body yields
   `embeddings_computed == 1`, `chunks_added == 1`, `chunks_removed == 1`, and
   `files_indexed == 1`. Run and confirm it fails. Implement diff-driven update
   and per-file hash reconciliation. Confirm it passes. Commit.
10. Write a failing test for rename: a commit renaming a file with unchanged
    content yields `embeddings_computed == 0` and `chunks_added == 0`, and every
    affected row's `file` field equals the new path. Run and confirm it fails.
    Implement the re-key path. Confirm it passes. Commit.
11. Write a failing test for delete: a commit deleting a file tombstones exactly
    its chunks, `chunks_removed` equals that count, `stats().dead` increases by
    it, and those rows are absent from `rows_for_file`. Run and confirm it
    fails. Implement deletion handling. Confirm it passes. Commit.
12. Write a failing test for reordering: a commit that moves two functions
    within a file without changing their bodies yields `embeddings_computed == 0`
    and leaves the live count unchanged. Run and confirm it fails. Confirm
    reconciliation is by hash set and not by order. Confirm it passes. Commit.
13. Write a failing test for `set_indexed_commit`: after `update`,
    `indexed_commit` equals the new head; after an `update` forced to fail
    mid-run, `indexed_commit` still equals the previous base. Run and confirm it
    fails. Move the call after the final commit. Confirm it passes. Commit.
14. Write failing tests for `DirtyPolicy`: with `IndexWorktree` and a dirty
    tree, the recorded commit carries the dirty marker and a subsequent `update`
    fully reconciles the dirty files; with `Refuse`, a dirty tree returns an
    error naming the dirty files. Run and confirm they fail. Implement the
    policy. Confirm they pass. Commit.
15. Write a failing test for compaction: tombstone enough chunks to cross a
    threshold of 0.20, run `update`, and assert `compacted` is `Some`, the live
    count is unchanged, `verify` returns `Ok`, and every surviving record is
    intact. Run and confirm it fails. Implement the end-of-run threshold check.
    Confirm it passes. Commit.
16. Write a failing test for parse failure: a commit introducing a syntax error
    in an indexed file increments `files_unparsed` and leaves that file's
    existing rows live. Run and confirm it fails. Implement the leave-untouched
    rule. Confirm it passes. Commit.
17. Write a failing test for interruption: an `index_all` forced to fail after
    the third batch leaves the store openable and verifying, and a subsequent
    `index_all` completes the index to the same final state as an uninterrupted
    run. Run and confirm it fails. Confirm resumability. Confirm it passes.
    Commit.
18. Add the `index_repo` example with `--timing` and `--report-vram`, and the
    `replay_commits` example which checks out the last N commits in order,
    running `update` at each and printing per-commit wall time and
    `embeddings_computed`. Add a progress printer implementing `Progress`.
    Commit.
19. Human step: run `cargo run --release -p indexing --example index_repo --
    <repo-path> --timing` on a repository of at least 50,000 lines and report
    wall-clock time, chunk count, store size, and the parse/embed/store split.
20. Human step: run `cargo run --release -p indexing --example replay_commits --
    <repo-path> --count 10` and report per-commit wall time and
    `embeddings_computed`, confirming cost tracks the diff.
21. Human step: run `cargo run --release -p indexing --example index_repo --
    <repo-path> --report-vram` with a desktop session attached and report peak
    GPU memory.
22. Run the full validation suite and confirm it passes.

## Validation

- **Unit:** `RepoGit` change classification for add, modify, delete, rename, and
  dirty detection; counter reconciliation.
- **Integration:** full index against a fixture with a known symbol set; no-op
  re-index; each of the five change kinds through `update`; compaction crossing
  the threshold; parse failure; interruption and resume. All using
  `MockEmbedder`, so the whole suite runs on CPU-only CI.
- **Regression:** `ChunkStore::verify` returns `Ok` after every test that
  mutates the store; the SIFT-003 per-language snapshots must remain unchanged,
  since a chunking change would invalidate every index.
- **Manual:** a full index of a real repository, then ten commits replayed;
  correct means the replay's embedding counts are small and proportional to each
  diff rather than to the repository.
- **Measurement:** full-index wall time, chunk count, store size, and the
  parse-versus-embed-versus-store split on a repository of at least 50,000
  lines, three runs, individual values and median; per-commit update wall time
  and `embeddings_computed` over ten commits; peak GPU memory during a full
  index with a desktop session attached.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
./ci.sh
cargo run --release -p indexing --example index_repo -- <repo-path> --timing
cargo run --release -p indexing --example replay_commits -- <repo-path> --count 10
cargo run --release -p indexing --example index_repo -- <repo-path> --report-vram
```

## Handoff

Report full-index wall time, total chunks, store size on disk, and the
parse/embed/store millisecond split over three runs with individual values and
the median; the embedding batch size and parse thread count used; per-commit
wall time and `embeddings_computed` for ten replayed commits, and specifically
that the single-function-edit commit computed one embedding and that the rename
commit computed none; the counts of files excluded, unsupported, and unparsed on
the real repository; `chunks_truncated`, since a non-trivial figure indicates
SIFT-003's oversize threshold needs revisiting; whether compaction triggered
during the replay and its report if so; confirmation that `ChunkStore::verify`
returned `Ok` after every run; and peak GPU memory during the full index with a
desktop session attached against the ~5.0 GB budget.
