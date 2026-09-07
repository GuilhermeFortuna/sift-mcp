# UI-002: Local console service

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Project direction:** [`../../tech-stack.md`](../../tech-stack.md), [`../../cuda-mcp-rtx2060-plan.md`](../../cuda-mcp-rtx2060-plan.md)  
**Depends on:** UI-001  
**Implementation plan:** [`../plans/UI-002-console-service-plan.md`](../plans/UI-002-console-service-plan.md)

## Purpose

Sift runs separate daemons per store and browsers cannot access their protected
sockets. A local service will register multiple repositories, bridge explicit
operations, and collect metadata history while remaining available when the
daemons are stopped.

## Requirements

### Registration and locality

- Registrations contain a display name and explicit repository, store, model,
  and daemon-executable locations. Equivalent store paths cannot be registered
  twice; missing or invalid locations receive actionable validation errors.
- Registration and passive collection never start a daemon or index a
  repository. Removing a registration deletes no repository or index files.
- Operations resolve a registered identifier to saved locations; the browser
  cannot supply an arbitrary path for an operation outside registration.
- The service listens only on loopback and enforces Host, Origin, same-origin,
  and action-forgery protections. It exposes only its bundled frontend assets.

### Collection and history

- Observe registered daemons every two seconds while the service runs. Distinguish
  stopped, unreachable, incompatible, and daemon lifecycle states; observations
  retain their timestamp and become visibly stale after collection failures.
- Retain metadata for seven days, at most 100,000 request records and 100,000
  metric samples globally, and at most 100 indexing reports per repository.
  Oldest entries are pruned first and history survives service restart.
- Duplicate events are not inserted after reconnection. Buffer loss, daemon
  restart, and collection outages are explicit coverage gaps; activity while
  the console was stopped is not promised to be complete.
- History contains no query text, pasted code, symbol body, or raw error
  payload. Persistence failures leave daemon retrieval operational and surface
  a history-recording error through live service health.
- Metrics describe daemon-side requests and identify their sample count and
  coverage. Missing GPU data and missing latency data are not zero.

### Explicit operations and browser delivery

- Explicit start, search, and indexing actions may launch the configured daemon.
  Protocol incompatibility never triggers process replacement.
- Indexing is tracked independently of browser connections. Disconnecting or
  refreshing a browser does not cancel a job; interrupted work is not success.
- Search, similar-code search, and symbol retrieval preserve existing daemon
  semantics, errors, result order, scores, and argument limits.
- Live updates deliver status, activity, and indexing changes. Reconnection
  obtains a fresh snapshot and cannot silently hide missed events.
- The frontend build foundation supports static production assets without a
  JavaScript server runtime. Ordinary Rust builds need no frontend assets.

## Constraints and non-goals

- No full product screens in this task, external monitoring service, remote
  hosting, multi-user authentication, scheduled indexing, or filesystem browser.
- No automatic daemon termination, model download, or store deletion.
- No resident inference dependencies in the console or changes to MCP tools.

## Acceptance criteria

### Agent-verifiable

1. Two registrations are isolated; aliases, invalid paths, missing identifiers,
   and duplicate stores are tested; registration performs no daemon launch.
2. History survives restart, deduplicates events, obeys every retention bound,
   marks gaps, and excludes seeded sensitive input.
3. Database failure is visible while explicit search remains operational.
4. Browser disconnect does not cancel indexing; concurrent indexing is refused;
   process interruption and incompatibility never appear successful.
5. Cross-origin actions, invalid Hosts, path traversal, and unregistered
   operation paths are rejected in integration tests.
6. HTTP search output matches daemon records and a production asset smoke test
   passes without a JavaScript server process.
7. The full project validation suite passes, including frontend foundation checks.

### Human-verifiable

1. None added by this service-only task. Target-hardware observation remains
   with UI-001; complete-console usability and overhead are assessed in UI-005.
