# UI-001 implementation plan: Daemon observability

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/UI-001-daemon-observability-spec.md`](../specs/UI-001-daemon-observability-spec.md)  
**Depends on:** SIFT-010, SIFT-011, SIFT-015

## Current-system context

`crates/daemon/src/protocol.rs` defines protocol version 1, a 1 MiB frame limit,
`DaemonStatus`, `IndexReportWire`, and streaming progress. `SharedState::status`
reports GPU bytes as zero and masks some missing store data with zeros.
`server.rs` touches idle time before dispatch and on disconnect, and counts
every connection when deciding whether to exit. Merely polling status therefore
changes daemon lifetime. `handle_hello` and `DaemonClient::connect` negotiate a
version before ordinary requests; preserve their wire-compatible negotiation.

Reuse `retrieval::StageTimings`, `IndexReportWire`, and existing request routing.
`inference::Embedder` currently has no resource method; `OnnxEmbedder`'s manually
set peak-byte field is not a live measurement. Extend these boundaries rather
than importing GPU dependencies into the daemon protocol or thin MCP client.
Re-read SIFT-015's final implementation after its dependency is satisfied.

## Interfaces produced

```rust
// crates/daemon/src/protocol.rs; declarations only
pub enum ClientRole { Worker, Observer }
pub enum Lifecycle { Starting, Ready, Indexing, Stale, ShuttingDown }
pub struct EventCursor { pub instance_id: String, pub sequence: u64 }
pub struct RequestEvent {
    pub cursor: EventCursor,
    pub connection_id: u64,
    pub request_id: u64,
    pub completed_at_unix_ms: u64,
    pub operation: String,
    pub elapsed_micros: u64,
    pub outcome: String,
    pub error_code: Option<String>,
    pub result_count: Option<u64>,
    pub stage_millis: Option<retrieval::StageTimings>,
}
pub struct ResourceSnapshot {
    pub sampled_at_unix_ms: u64,
    pub device_id: Option<String>,
    pub device_used_bytes: Option<u64>,
    pub device_total_bytes: Option<u64>,
    pub process_used_bytes: Option<u64>,
    pub model_used_bytes: Option<u64>,
}
pub struct Observation {
    pub status: DaemonStatus,
    pub events: Vec<RequestEvent>,
    pub next_cursor: EventCursor,
    pub gap: bool,
    pub more: bool,
}
// Append Request::Observe { after: Option<EventCursor> } and
// Response::Observation(Observation); do not reorder existing variants.
// Extend DaemonStatus with lifecycle, instance_id, observed_at_unix_ms,
// current progress, last index completion, and ResourceSnapshot.
// Make unavailable model/count/commit metadata optional; remove fabricated
// resident_gpu_bytes in protocol v2. The MCP SearchResult shape is unchanged.

// crates/daemon/src/client.rs
impl DaemonClient {
    pub async fn connect_observer(socket_path: &std::path::Path)
        -> Result<Self, DaemonError>;
    pub async fn observe(&mut self, after: Option<EventCursor>)
        -> Result<Observation, DaemonError>;
}

// crates/inference/src/embedder.rs
pub struct ResourceUsage { /* same resource fields, independent of daemon */ }
// Add object-safe Embedder::resource_usage with a default unavailable result.
// GPU implementation remains in inference under cuda.
```

Progress snapshots carry phase, done, optional total, and instance/connection/
request identity. Last-index completion additionally carries completion time,
safe outcome/error code, and optional existing `IndexReportWire`. Keep these
separate from query metadata so no symbol or query payload is retained.

## Implementation decisions

- Use version 2 while retaining the exact version-1 Hello and ProtocolVersion
  encodings, because a new observer must diagnose an old daemon before sending
  newly added variants. Reserve Hello client value `sift-console-observer` as
  the distinct observer handshake; all other clients remain workers. This is a
  restriction of capabilities, not a new authentication boundary.
- Classify only after Hello and exempt provisional handshakes and observers from
  worker counts/touches, because accepting a socket alone says nothing about its
  role. Give provisional handshakes a five-second deadline. Worker activity
  preserves existing residency semantics. Observers may only Observe or Status.
- Permit observer handshake before stale/startup worker rejection, because those
  are precisely the states the console must diagnose. Close observers explicitly
  on shutdown rather than awaiting their next request indefinitely.
- Keep a 4,096-event ring behind a short lock, because a slow collector must not
  backpressure inference. Give each daemon a random instance ID, each connection
  a monotonic ID, and each terminal event a monotonic sequence. Return at most
  256 events per Observe and also enforce the existing encoded frame ceiling;
  `more` pages the remainder and `gap` identifies evicted/mismatched cursors.
- Record worker Search, SearchSimilar, GetSymbol, and Index operations exactly
  once at terminal completion, including failures. Use monotonic elapsed time
  from dispatch admission through computation; exclude response socket delivery
  and label this daemon-side time. Track disconnect delivery failures separately
  from successful computation. Retain only fixed operation/outcome/error codes.
- Update progress in shared diagnostic state before attempting delivery to its
  initiating client, because observers must see external indexing independently.
  Snapshot metadata must not wait for the indexing owner lock.
- Cache resource samples at a maximum rate of one per two seconds. Add an
  optional dynamically loaded NVML adapter inside `inference`'s CUDA feature:
  use the configured device UUID and current PID, returning unavailable fields
  on unsupported access. Do not use the mutable peak-byte placeholder or equate
  device usage with model allocation. Model attribution stays unavailable until
  a real allocator-level measurement exists.
- Add an instrumentation-enable configuration switch, default on, for paired
  measurement; disabling recording leaves passive status and role semantics
  unchanged. No expensive persistence or raw payload formatting on the hot path.

## Ordered implementation

1. Check the ledger dependencies, read the complete spec, and create branch
   `UI-001-daemon-observability`. Do not implement if any dependency is incomplete.
2. Write protocol regression tests: version-1 negotiation yields a named mismatch,
   version 2 works, and existing search-result snapshots remain identical. Run
   and confirm they fail for the new contract. Implement negotiation and types,
   update workspace clients/fixtures together, confirm they pass, and commit.
3. Write tests with a one-second idle timeout and repeated observer reads every
   50 ms: exit within two seconds with no workers; denied observer Search/Index/
   GetSymbol/Shutdown and observer disconnect never extend idle time. Add slow
   startup, stale state, and incomplete-handshake tests. Run and confirm they
   fail. Implement role accounting and observer shutdown, pass tests, and commit.
4. Write tests for two connections both using request 2, one successful search,
   one error, an initiating-client disconnect, and a completed external index.
   Assert one terminal event per operation, distinct identities, current progress,
   safe failure codes, and absence of seeded query/code/path strings. Run and
   confirm they fail. Implement metadata capture, pass tests, and commit.
5. Write tests inserting 4,100 events: retain 4,096, signal a cursor gap, page at
   most 256, remain below 1 MiB, and never replay the last consumed sequence.
   Restart fixtures must change instance ID. Run and confirm they fail.
   Implement bounded observation and slow-observer isolation, pass, and commit.
6. Write mock-resource tests: all GPU values unavailable by default; measured
   zero stays zero; shared device identity survives serialization; failed sampling
   cannot fail a search. Run and confirm they fail. Add cached inference sampling
   and the optional adapter, run CPU tests and CUDA compile checks, and commit.
7. Add `scripts/measure-ui-observability.sh` backed by a Rust measurement example
   in the daemon crate. Its CLI accepts repo/store/model/daemon paths, three runs,
   and output directory; it runs fixed fixture queries after equal warmup with
   recording off/on, reports per-run daemon p50/p95, process RSS, available device
   and process VRAM, device UUID, sample count, and medians. Add CPU tests for
   report aggregation (nearest-rank p95 of 1..100 is 95) and argument validation;
   confirm they fail before implementing the helper, pass, and commit.
8. Human step: run the resource/paired-measurement procedure below with a desktop
   attached; compare device samples against `nvidia-smi`, and preserve individual
   reports and overhead even if unfavorable. Leave both human criteria pending
   until owner evidence is recorded.
9. Run `./ci.sh`, inspect the diff for unrelated changes, record validation and
   outstanding human evidence in the handoff, and stop. Do not begin UI-002.

## Validation

- Unit/integration: all explicit assertions above with MockEmbedder and fake clocks
  where possible; socket shutdown retains one real-time integration test.
- Regression: locked MCP search serialization, search/index concurrency, worker
  residency, and mcp-client dependency closure.
- Measurement: three paired runs on the same store/model/queries after identical
  warmup, with recording mode the only changed setting. These are observational
  figures, not replacement corpora or SIFT-013 acceptance measurements.

Implementation and human commands (environment variables name owner-supplied
absolute paths; they are not read from secret configuration):

```bash
./ci.sh
scripts/measure-ui-observability.sh --repo "$SIFT_REPO" --store "$SIFT_STORE" --model "$SIFT_MODEL" --daemon "$SIFT_DAEMON" --runs 3 --output "$SIFT_UI_REPORT_DIR"
nvidia-smi --query-gpu=uuid,memory.used,memory.total --format=csv
```

## Handoff

Report protocol migration, observer idle-shutdown evidence, ring/frame bounds,
privacy tests, CPU/CUDA-compile validation boundaries, and all human measurement
results or explicit outstanding criteria. Never mark hardware acceptance from
mock data. Task status remains solely in the ledger.
