# SIFT-010 implementation plan: Resident daemon

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/SIFT-010-resident-daemon-spec.md`](../specs/SIFT-010-resident-daemon-spec.md)  
**Depends on:** SIFT-006, SIFT-009

## Current-system context

`crates/daemon` is empty from SIFT-001. Everything it must host exists as
in-process components: `indexing::Indexer` (SIFT-006) with `index_all`,
`update`, an `IndexReport`, and a `Progress` trait built so a caller can stream
it; `retrieval::Searcher` (SIFT-009) with `search`, `search_similar`, and
`SearchDiagnostics` carrying `StageTimings`; `retrieval::DenseIndex::refresh`
(SIFT-008), which exists precisely so that something outside the indexer decides
when a search sees new rows; `storage::ChunkStore` (SIFT-002), documented as
single-writer with concurrent readers under SQLite WAL; and
`inference::OnnxEmbedder` (SIFT-005) with `peak_gpu_bytes` and distinguishable
`GpuUnavailable`, `ModelFilesMissing`, and `Allocation` errors.

`tokio` is named in `docs/tech-stack.md` but is not yet a workspace dependency,
and nothing in the repository opens a socket or runs longer than one command.
The gap this task closes is that every component is loaded and dropped per
invocation, which the design document identifies as a 20–40 second cost against
a 400 ms budget.

## Interfaces produced

```rust
// crates/daemon/src/protocol.rs
/// Bumped on any wire-incompatible change. Checked during Hello.
pub const PROTOCOL_VERSION: u32 = 1;
/// Requests above this are rejected without being buffered.
pub const MAX_REQUEST_BYTES: usize = 1 << 20;

pub struct Envelope<T> {
    pub request_id: u64,
    pub payload: T,
}

pub enum Request {
    Hello { protocol_version: u32, client: String },
    Search { query: String, top_k: usize },
    SearchSimilar { code: String, top_k: usize },
    GetSymbol { file: String, symbol: String },
    Index { mode: IndexMode },
    Status,
    Shutdown,
}

pub enum IndexMode { Full, Update }

pub enum Response {
    Hello { protocol_version: u32, model_id: String, chunks: u64 },
    Search(retrieval::SearchResponse),
    Symbol { file: String, symbol: String, language: String,
             signature: String, lines: [u32; 2], body: String },
    /// Streamed while an Index request runs; terminal frame is IndexDone.
    IndexProgress { phase: indexing::Phase, done: u64, total: Option<u64> },
    IndexDone(indexing::IndexReport),
    Status(DaemonStatus),
    Error(DaemonError),
}

/// Typed so the client can act differently per cause rather than parse strings.
pub enum DaemonError {
    ProtocolVersion { daemon: u32, client: u32 },
    Starting,                      // model still loading; retry
    IndexInProgress,               // writer busy; retry
    SymbolNotFound { file: String, symbol: String },
    SymbolAmbiguous { file: String, symbol: String, candidates: Vec<String> },
    StoreStale { reason: String }, // store replaced under the daemon
    GpuUnavailable { detail: String },
    RequestTooLarge { bytes: usize, limit: usize },
    Malformed { detail: String },
    Internal { detail: String },
}

pub struct DaemonStatus {
    pub model_id: String,
    pub chunks_live: u64,
    pub chunks_dead: u64,
    pub indexed_commit: Option<String>,
    pub indexing: bool,
    pub resident_gpu_bytes: u64,
    pub idle_seconds: u64,
    pub uptime_seconds: u64,
}
```

```rust
// crates/daemon/src/server.rs
pub struct DaemonConfig {
    pub store_dir: PathBuf,
    pub model_dir: PathBuf,
    pub socket_path: PathBuf,     // derived from store_dir; see decisions
    pub idle_timeout: Duration,   // default 15 minutes
    pub max_concurrent_searches: usize,
    pub fusion: retrieval::FusionConfig,
}

pub struct Daemon { /* listener, lock file, shared state, task tracker */ }

impl Daemon {
    /// Binds, acquires the single-instance lock, then loads model and indexes.
    /// Serves Status and Error::Starting while loading.
    pub async fn bind(config: DaemonConfig) -> Result<Self, DaemonError>;
    pub async fn serve(self) -> Result<(), DaemonError>;
}

/// Shared read-mostly state. Search borrows it; indexing swaps it.
pub struct Resident {
    /* ChunkStore, LexicalIndex, DenseIndex, OnnxEmbedder, LiveMask */
}
```

```rust
// crates/daemon/src/client.rs
/// Used by SIFT-011. Lives here so protocol types have exactly one definition.
pub struct DaemonClient { /* unix stream, request id counter */ }

impl DaemonClient {
    /// Connects, or spawns a daemon and retries with backoff until the deadline.
    pub async fn connect_or_spawn(store_dir: &Path, deadline: Duration)
        -> Result<Self, DaemonError>;
    pub async fn request(&mut self, req: Request) -> Result<Response, DaemonError>;
    /// Yields IndexProgress frames until IndexDone.
    pub async fn request_streaming(&mut self, req: Request)
        -> Result<impl Stream<Item = Response>, DaemonError>;
}
```

## Implementation decisions

- **The socket path is derived from the canonicalized store directory by hash
  and placed in the user's runtime directory, not inside the store.** A socket
  inside the store would be walked by the indexer and synced by tools that copy
  the store; deriving it from the path is what makes "one daemon per store"
  automatic rather than a convention.

- **Single-instance enforcement is an exclusive lock on a lock file beside the
  socket, acquired before binding, with a stale socket unlinked only after the
  lock is held.** Checking whether the socket is connectable and then binding is
  a race that two simultaneous first-connections will lose, leaving one daemon
  bound to an unlinked socket that no client can reach.

- **The socket is created with permissions restricting it to the owning user,
  and the directory containing it is checked to be owner-only.** The spec makes
  filesystem permissions the entire security boundary; a socket in a
  world-writable directory can be replaced by another user's socket, which is a
  full compromise of everything the daemon returns.

- **`bind` binds and starts serving *before* the model finishes loading, and
  answers `Status` and `DaemonError::Starting` until it is ready.** If binding
  waited for the model, the client's connect-or-spawn would have to distinguish
  "not started" from "starting" by timing, and the first request after a cold
  start would fail rather than wait.

- **Framing is a length prefix followed by a serialized envelope, with the
  length checked against `MAX_REQUEST_BYTES` before allocating.** Reading a
  declared length into a buffer before checking it is how a single malformed
  frame takes the daemon down with an allocation failure.

- **`PROTOCOL_VERSION` is checked in `Hello`, before any other request is
  accepted.** A mismatched client that is allowed to send a `Search` first has
  already had its bytes interpreted under the wrong schema, and the error it
  gets back describes a parse failure rather than the version mismatch.

- **`Resident` is held behind a read-write lock and indexing swaps a freshly
  prepared value in, rather than mutating in place.** SIFT-008's `DenseIndex`
  must be `refresh`ed and SIFT-002's `LiveMask` rebuilt after an update; doing
  that in place means a concurrent search can observe a mask that disagrees with
  the matrix. Swapping means in-flight searches finish against the old value and
  new ones see the new one, and the writer holds the write lock only for the
  swap.

- **Only one index operation runs at a time, enforced by a mutex the request
  handler tries without blocking, returning `IndexInProgress`.** The store is
  single-writer by SIFT-002's contract. Blocking instead of refusing would let a
  client queue up an unbounded number of index requests behind a long one.

- **Searches during an index are served from the pre-swap `Resident` and never
  wait on the indexer.** This is the direct reason for the swap design: the spec
  requires that indexing not block searches for its duration, and a shared
  mutable state would serialize them.

- **`GetSymbol` reads the body from the lexical index rather than from disk.**
  The indexed revision is what the search results describe; reading the current
  file would return a body that does not match the line numbers just reported if
  the file changed since indexing.

- **`SymbolAmbiguous` carries the candidate names.** An agent told only that a
  symbol is ambiguous has no way forward; given the candidates it can pick one,
  which is what makes the error actionable per the spec.

- **Concurrent searches are bounded by a semaphore sized to
  `max_concurrent_searches`, defaulting to 4, and the wait is inside the
  request rather than an admission refusal.** The GPU serializes anyway; the
  bound exists to keep queued work from multiplying VRAM through concurrent
  inference batches. It is not a queue with priorities, which the spec rules
  out.

- **The idle timer counts from the last completed request with no connected
  clients, and shutdown is graceful: stop accepting, await in-flight tasks, drop
  `Resident`, release the lock, unlink the socket.** Dropping `Resident` is what
  actually returns the VRAM; unlinking last means a client that connects during
  shutdown gets a connection error and respawns, rather than connecting to a
  daemon that is tearing down.

- **Store staleness is detected by comparing the store's identity and schema
  version at each `Resident` swap and on a periodic tick, surfacing
  `StoreStale`.** A daemon holding a memory map of a deleted-and-recreated store
  serves results describing chunks that no longer exist, with no error anywhere.

- **Every request logs one structured line with request id, type, outcome, and
  `StageTimings`.** The 400 ms SLO is defended by SIFT-013 with measurements
  taken through this path, and a summary that is not per-request cannot show a
  95th percentile.

- **`DaemonClient` lives in this crate rather than in the MCP crate.** The
  protocol types would otherwise be defined twice or exported from a third
  crate, and SIFT-011's crate must stay free of anything heavy — a client that
  shares this crate's types costs it nothing at runtime because the protocol
  module has no model or index dependency.

## Ordered implementation

1. Create the branch `SIFT-010-resident-daemon`.
2. Declare `tokio` with the needed features, `serde`, `bincode` or an equivalent
   framed codec, `tracing`, `tracing-subscriber`, and `fs4` for file locking in
   `[workspace.dependencies]`; inherit them in `crates/daemon` and depend on
   `storage`, `indexing`, `retrieval`, and `inference` with default features so
   the crate builds without a GPU. Confirm `./ci.sh` passes. Commit.
3. Write failing unit tests for framing: a frame at exactly `MAX_REQUEST_BYTES`
   is accepted; one byte larger is rejected with `RequestTooLarge` naming both
   numbers and without allocating the declared length, asserted by a bounded
   allocator or by rejecting before read; a truncated frame yields `Malformed`;
   after either rejection the codec can read a subsequent valid frame. Run and
   confirm they fail. Implement the codec. Confirm they pass. Commit.
4. Write failing tests for `Hello`: a client at `PROTOCOL_VERSION` succeeds and
   receives the model id and chunk count; a client at a different version
   receives `ProtocolVersion` naming both; any request before `Hello` is
   refused. Run and confirm they fail. Implement the handshake. Confirm they
   pass. Commit.
5. Write failing tests for socket placement and permissions: the socket path is
   deterministic for a given canonicalized store directory and differs for a
   different store; the socket's mode denies group and other; binding in a
   world-writable directory is refused. Run and confirm they fail. Implement
   path derivation and permission checks. Confirm they pass. Commit.
6. Write a failing test for single instance: two concurrent `bind` calls for the
   same store produce exactly one listener, the loser observes the lock is held,
   and a client connects to the winner. Add a test that a stale socket left by a
   killed daemon is cleaned up and rebound. Run and confirm they fail. Implement
   lock-then-unlink-then-bind. Confirm they pass. Commit.
7. Write a failing integration test for search over the socket using
   `MockEmbedder`: a `Search` request returns results equal to calling
   `Searcher::search` in process on the same store. Run and confirm it fails.
   Implement `Resident` construction and the search handler. Confirm it passes.
   Commit.
8. Write a failing test that `Status` and `Error::Starting` are served while the
   model is still loading, using a deliberately slow loader. Run and confirm it
   fails. Move loading off the bind path. Confirm it passes. Commit.
9. Write a failing concurrency test: two clients issue searches whose handlers
   record entry and exit timestamps, and the intervals are asserted to overlap.
   Run and confirm it fails. Confirm per-connection task spawning. Confirm it
   passes. Commit.
10. Write a failing test for `GetSymbol`: a known symbol returns its body from
    the lexical index with the same line range search reported; an absent symbol
    returns `SymbolNotFound`; a file with two same-named symbols returns
    `SymbolAmbiguous` listing both qualified candidates. Run and confirm they
    fail. Implement the handler. Confirm they pass. Commit.
11. Write a failing test for index streaming: an `Index` request emits
    `IndexProgress` frames for each phase and terminates with `IndexDone`
    carrying an `IndexReport` equal to the in-process one. Run and confirm it
    fails. Implement `Progress` forwarding over the socket. Confirm it passes.
    Commit.
12. Write a failing test for search-during-index: with a deliberately slowed
    indexer, searches issued throughout return successfully, none observes a
    partial view, and results before the swap match the pre-index state while
    results after match the post-index state. Run and confirm it fails.
    Implement the prepare-then-swap of `Resident`, including `DenseIndex::refresh`
    and `LiveMask` rebuild. Confirm it passes. Commit.
13. Write a failing test that a second concurrent `Index` request returns
    `IndexInProgress` rather than running or blocking. Run and confirm it fails.
    Implement the try-lock. Confirm it passes. Commit.
14. Write a failing test for idle shutdown: with a one-second idle timeout, the
    daemon exits after the period with no clients, and a subsequent
    `connect_or_spawn` starts a new one and succeeds. Run and confirm it fails.
    Implement the idle timer and graceful shutdown ordering. Confirm it passes.
    Commit.
15. Write a failing test for graceful shutdown under load: a `Shutdown` sent
    while a slow search is in flight completes that search before exiting, and
    `ChunkStore::verify` passes afterwards. Run and confirm it fails. Implement
    task tracking and awaiting. Confirm it passes. Commit.
16. Write a failing test for `StoreStale`: replacing the store directory under a
    running daemon causes subsequent requests to return `StoreStale`. Run and
    confirm it fails. Implement identity checking. Confirm it passes. Commit.
17. Write a failing test asserting every request emits one structured log line
    carrying request id, type, outcome, and stage timings, captured through a
    test subscriber. Run and confirm it fails. Implement the tracing span.
    Confirm it passes. Commit.
18. Write a failing test for `GpuUnavailable`: a daemon configured with an
    unavailable GPU reports it as a typed error on request rather than falling
    back or exiting silently. Run and confirm it fails. Implement propagation.
    Confirm it passes. Commit.
19. Add `scripts/time-daemon-start.sh`, `scripts/report-daemon-vram.sh`,
    `scripts/observe-idle-eviction.sh`, and `scripts/index-under-load.sh`.
    Commit.
20. Human step: run `scripts/time-daemon-start.sh <store-path>` on the target
    machine with the real model and a full-size index, over five runs, and
    report the time to first served search.
21. Human step: run `scripts/report-daemon-vram.sh <store-path>` with a desktop
    session attached and report resident GPU memory against the ~5.0 GB budget.
22. Human step: run `scripts/observe-idle-eviction.sh <store-path>` and confirm
    the daemon exits after the idle period and GPU memory is released.
23. Human step: run `scripts/index-under-load.sh <repo-path>` and report search
    latencies observed during indexing and confirmation that progress was
    visible throughout.
24. Run the full validation suite and confirm it passes.

## Validation

- **Unit:** framing limits and recovery; `Hello` version checking; socket path
  derivation and permissions.
- **Integration:** search over the socket equal to in-process; serving during
  model load; concurrent clients; `GetSymbol` found, absent, and ambiguous;
  index progress streaming; search-during-index consistency across the swap;
  concurrent index refusal; idle shutdown and respawn; graceful shutdown under
  load; store staleness; per-request logging; GPU-unavailable propagation. All
  with `MockEmbedder` so they run on CPU-only CI.
- **Regression:** `Searcher::search` over the socket must equal the in-process
  result for the same store — the socket adds transport, not behaviour;
  `ChunkStore::verify` passes after every test that indexes.
- **Manual:** idle eviction observed against GPU memory; indexing under
  concurrent search load; correct means searches keep succeeding and progress
  frames keep arriving.
- **Measurement:** daemon start to first served search with the real model and a
  full-size index, five runs, individual values and median; resident GPU memory
  with a desktop session attached; search latency during an index compared
  against the idle figures from SIFT-009.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
./ci.sh
scripts/time-daemon-start.sh <store-path>
scripts/report-daemon-vram.sh <store-path>
scripts/observe-idle-eviction.sh <store-path>
scripts/index-under-load.sh <repo-path>
```

## Handoff

Report daemon start time to first served search over five runs with individual
values and the median, and the split between model load and index open; resident
GPU memory with the embedder loaded and a full-size index open, measured with a
desktop session attached, against the ~5.0 GB budget; confirmation that the
daemon exited after the idle period and that GPU memory was released, with the
observed timing; search latency measured during an active index compared against
the idle figures from SIFT-009, and whether the swap design held searches
unblocked; the configured `max_concurrent_searches` and `idle_timeout` values;
confirmation that two simultaneous binds produced exactly one listener and that
a stale socket was reclaimed; the socket path derivation and its permissions;
and confirmation that `ChunkStore::verify` passed after indexing through the
daemon.
