# UI-002 implementation plan: Local console service

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/UI-002-console-service-spec.md`](../specs/UI-002-console-service-spec.md)  
**Depends on:** UI-001

## Current-system context

The workspace has Tokio, rusqlite, and `daemon` with resident dependencies behind
features. `DaemonClient::connect_or_spawn` accepts explicit store/repo/model/
binary paths, and `request_streaming` forwards index progress. Reuse these paths
for explicit actions, but use UI-001's observer connection for collection.
`SearchResponse`, `StageTimings`, and `IndexReportWire` already describe results.

There is no HTTP service, registration registry, frontend, or historical metrics
store. UI-001 supplies the planned `Observation` and stable event cursor; read
its completed interfaces before starting. This task provides all backend routes
needed by later screens and a minimal buildable frontend, without designing the
product screens prematurely.

## Interfaces produced

Use `crates/console` (binary `sift-console`) with modules `api`, `registry`,
`history`, `collector`, `jobs`, `security`, and `assets`; frontend lives in `ui`.
The console depends on `daemon` with `default-features = false`, never resident
inference. Define API DTOs in `api/types.rs` and matching TypeScript declarations
in `ui/src/api/types.ts`; golden JSON contract tests cover both consumers.

```rust
// crates/console/src/api/types.rs; declarations only
pub struct RegistrationInput {
    pub name: String,
    pub repo_path: std::path::PathBuf,
    pub store_path: std::path::PathBuf,
    pub model_path: std::path::PathBuf,
    pub daemon_path: std::path::PathBuf,
}
pub struct Registration { pub id: String, pub config: RegistrationInput }
pub struct ApiError { pub code: String, pub message: String, pub retryable: bool }
pub struct Page<T> { pub items: Vec<T>, pub next_cursor: Option<String> }
pub enum JobState { Running, Succeeded, Failed, Interrupted }
pub struct IndexJob {
    pub id: String,
    pub repository_id: String,
    pub state: JobState,
    pub progress: Option<daemon::IndexPhase>,
    pub done: u64,
    pub total: Option<u64>,
    pub report: Option<daemon::IndexReportWire>,
    pub error_code: Option<String>,
}
// crates/console/src/lib.rs
pub struct ConsoleConfig { /* loopback address, database and asset paths */ }
pub async fn serve(config: ConsoleConfig) -> Result<(), Box<dyn std::error::Error>>;
```

All routes below are prefixed `/api/v1`. IDs are opaque UUIDs, timestamps UTC
Unix milliseconds, durations explicitly microseconds or milliseconds as named.

| Method and route | Input and output |
| --- | --- |
| GET `/session` | In-memory CSRF token; no-store response |
| GET `/health` | Collector/persistence health and observation time |
| GET/POST `/repositories` | List registrations / validated RegistrationInput → 201 Registration |
| GET/PATCH/DELETE `/repositories/{id}` | Read / full validated replacement input / remove registration only |
| GET `/repositories/{id}/status` | Timestamped lifecycle, resources, current progress, latest report, connection state |
| GET `/repositories/{id}/freshness` | HEAD, indexed commit, dirty boolean or unavailable reason, inspected time |
| POST `/repositories/{id}/start` | Connect-or-spawn with configured paths; ready status or typed error |
| POST `/repositories/{id}/index` | Mode update/full → 202 IndexJob; conflicting work → 409 |
| GET `/repositories/{id}/jobs` | Running job and bounded recent report summaries |
| GET `/jobs/{id}` | IndexJob snapshot |
| POST `/repositories/{id}/search` | Query and top_k → existing SearchResponse |
| POST `/repositories/{id}/similar` | Code and top_k → existing SearchResponse |
| POST `/repositories/{id}/symbol` | Relative file and symbol → existing Symbol response |
| GET `/activity` | Repository/operation/outcome/from/to/cursor/limit → Page of safe RequestEvent records |
| GET `/metrics` | Repository/from/to → request counts, rate, p50/p95, resource series, coverage and sample counts |
| GET `/events` | SSE invalidations: status, activity, indexing, health, reset |

Activity pages default to 50, maximum 200; time windows default to one hour and
are limited to the retained seven days. Metrics use one-minute buckets,
nearest-rank percentiles of completed-operation durations, and null for empty
latency buckets. Rate is completed count divided by covered seconds, null when
coverage is absent; never interpolate across coverage gaps. Return coverage
seconds and gap markers so rate is not presented as uninterrupted traffic.

TypeScript names `ActivityPage` as the page of repository-tagged RequestEvent
records and `MetricsResponse` as the bucket series plus coverage/sample counts
described above. Export both from `ui/src/api/types.ts` for UI-003; preserve the
wire names rather than deriving a second screen-specific response schema.

## Implementation decisions

- Persist SQLite in `$XDG_STATE_HOME/sift-console/console.sqlite3` (fallback
  `~/.local/state`), with owner-only directory/file permissions and a singleton
  lock, because multiple collectors must not race the same cursor. Use WAL,
  schema-version migrations, foreign keys, and a dedicated blocking DB worker.
- Separate registration, request_events, metric_samples, index_reports,
  collection_gaps, and collector_cursors tables. Insert events and cursor
  advancement in one transaction because cursor advancement without persistence
  loses history. Unique key is repository plus instance plus event sequence.
- Apply seven-day expiry and global 100,000-row caps to events and samples on
  ingest and startup, and cap reports at 100 per repository. Cap gap records at
  10,000 globally and seven days; do not prune registrations. Prune oldest first
  with deterministic ID tie-breaking to keep tests and disk growth bounded.
- Canonicalize existing paths; a new store may have a missing final leaf under
  an existing canonical parent. Repo/model must be directories and daemon must
  be an executable file. Verify model artifacts when starting rather than loading
  them during registration. Reject blank names and non-absolute paths. Do not
  create the store during registration, because registration must stay passive.
- The collector uses two-second ticks, two-second observation deadlines, at
  most four concurrent repository reads and four pages per tick per repository.
  A delayed tick is skipped, not queued. Persistent cursors and instance changes
  expose gaps. Last good state remains timestamped and stale on failure.
- Registration removal cascades only console-owned history, never project files.
  Refuse edit/removal while a console-owned job is running. A changed config
  closes the old observer and starts fresh observation for that registration.
- Jobs own worker connections in the service, because a browser HTTP/SSE lifetime
  must not own indexing. Keep one job per repository; let daemon busy errors
  arbitrate external work. Persist terminal outcomes and running job identity;
  after console restart mark orphan jobs interrupted, then reconcile any current
  daemon indexing separately. Never retry Index automatically after lost replies.
- Explicit worker calls may launch through `connect_or_spawn`; use a 120-second
  start deadline, 60-second search/symbol response deadline, and no wall-time
  cancellation for an active index. Return safe typed errors on timeout; explain
  that a disconnected operation may still be running. Do not respawn on version
  mismatch. Symbol retrieval connects only to an existing daemon, because a
  stopped/replaced index should not be silently substituted for displayed hits.
- Freshness uses gix in a blocking worker, inspecting HEAD and working-tree
  changes on explicit request with a five-second timeout and five-second cache.
  Report untracked changes too; unreadable inspection yields unknown, not clean.
- Bind `127.0.0.1:7331` by default, with a configurable loopback-only port.
  Allow only its exact loopback Host and same Origin. Reject cross-site fetches;
  mutation requests require JSON and `X-Sift-CSRF` matching a random in-memory
  token from `/session`. Do not enable permissive CORS. Limit JSON bodies to
  1 MiB and ensure encoded daemon frames also fit before sending.
- Serve only the canonical configured asset directory, reject traversal and
  escaping symlinks, set a self-only CSP and no-store API headers. SPA fallback
  applies to HTML navigation outside `/api`, never missing API routes/assets.
- SSE uses bounded invalidations, not source content. Slow subscribers receive
  reset/reconnect behavior; initial connection always refreshes HTTP snapshots.
  Collection and history errors travel in memory so DB failures cannot hide them.
- Set up React/TypeScript/Vite, pnpm lockfile, Tailwind, and an empty application
  entry. Pin resolved compatible versions and package-manager/runtime versions
  at implementation. Vite proxies `/api` to Axum during local development with
  Origin/Host rewritten to the backend; production serves from one origin.
  No frontend build script in Cargo: assets are supplied by `--assets` or the
  executable-adjacent `ui` directory, and absent assets produce a clear error.

## Ordered implementation

1. Read the spec, confirm UI-001 dependency acceptance, create
   `UI-002-console-service`, and add only this task's implementation.
2. Write registry tests for two repositories, symlink aliases, a missing store
   leaf, invalid model/binary paths, duplicate store and removal preserving files.
   Run and confirm they fail. Implement registry/schema migration and tests for
   migration rollback on error, confirm they pass, and commit.
3. Write retention/dedup tests with an injected clock: expired rows disappear,
   inserting 100,001 events/samples retains 100,000, 101 reports retains 100,
   and reconnect inserts no duplicates. Seed private input and assert its absence.
   Run and confirm they fail. Implement history transactions/pruning, pass, commit.
4. Write collector tests for startup, stale, stopped, unreachable, version mismatch,
   ring overflow, restart, and DB-write failure. Assert no passive spawn and no
   cursor advance after a failed transaction. Run and confirm they fail.
   Implement collection, health, metric derivation, and SSE invalidation, pass,
   and commit. Assert p95 of durations 1..100 is 95 and no samples yields null.
5. Write API tests for Host/Origin/CSRF rejection, oversized requests, traversal,
   escaping symlink assets, unknown ID, and JSON error codes. Run and confirm
   they fail. Implement Axum routes/security, pass, and commit.
6. Write two-daemon action tests: browser disconnect during Index, external Index
   busy, duplicate submission, failed spawn, version mismatch without spawn,
   orphan-job restart, and search records equal to socket results. Run and
   confirm they fail. Implement job ownership, actions, safe errors, and freshness
   fixtures (clean/dirty/unborn/unknown), pass, and commit.
7. Add frontend foundation and golden API fixture tests; first confirm an absent
   endpoint/client contract fails. Implement typed fetch/session handling and the
   minimal app, then pass TypeScript/Vitest and asset-serving tests. Add all checks
   to `./ci.sh`, including frozen pnpm installation, bounded browser setup/runs,
   frontend lint/typecheck/test/build, without extra workflow steps. Commit.
8. Run `./ci.sh` on a CPU-only environment, confirm ordinary Cargo builds without
   `ui/dist`, report the exact boundary tested, and stop before UI-003.

## Validation

Unit and integration tests use temporary SQLite, fake clocks, and CPU mock
daemons; actual socket/API tests prove observer and worker paths differ. Golden
responses pin TypeScript/Rust agreement without introducing a GPU dependency.
Do not run real indexing against user repositories during automated tests.

```bash
./ci.sh
cargo build --workspace
```

The implementation adds this launch interface for later tasks:

```bash
cargo run -p console --bin sift-console -- --listen 127.0.0.1:7331 --assets ui/dist
```

## Handoff

Report registry/history migration and retention coverage, origin/CSRF tests,
job disconnect/restart behavior, generated asset smoke results, API contract
fixtures, and dependency closure. No human GPU result is required by this task;
do not infer real-model acceptance from mock daemons.
