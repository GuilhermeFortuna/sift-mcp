# UI-001: Daemon observability

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Project direction:** [`../../tech-stack.md`](../../tech-stack.md), [`../../cuda-mcp-rtx2060-plan.md`](../../cuda-mcp-rtx2060-plan.md)  
**Depends on:** SIFT-010, SIFT-011, SIFT-015  
**Implementation plan:** [`../plans/UI-001-daemon-observability-plan.md`](../plans/UI-001-daemon-observability-plan.md)

## Purpose

The resident daemon has basic status and per-request logging, but monitoring it
currently affects idle shutdown and cannot recover recent request activity.
Provide passive, truthful diagnostics covering requests from every client so a
local console can explain service health without changing service residency.

## Requirements

### Passive observation

- An observer can connect during startup, indexing, normal serving, and store
  staleness; observation never starts or reloads a model.
- Observer connections, diagnostics requests, and disconnects neither extend
  the idle timeout nor count as active worker clients.
- Observers cannot search, retrieve symbols, index, or shut down the daemon.
  Idle shutdown closes observer connections rather than waiting for them.
- Incompatible clients receive an actionable version mismatch before executing
  work; existing MCP tool names, arguments, ranking, and result records remain
  unchanged.

### State and activity

- Diagnostics distinguish startup, ready, indexing, stale, and shutdown, with
  observation time, daemon identity, uptime, idle time, model identity, live and
  dead chunk counts, and indexed commit. Unavailable values are explicit.
- Every completed worker operation has one metadata event carrying instance,
  connection, request, sequence, operation, monotonic duration, outcome, safe
  error category, result count when applicable, and available stage timings.
- Query text, pasted code, symbol bodies, paths supplied by requests, raw error
  payloads, and arbitrary client names are absent from retained diagnostics.
- Index progress and the latest completed indexing report are observable even
  when an MCP agent initiated indexing. Failure is never reported as success.
- A bounded recent-event buffer exposes sequence gaps. Restarted instances
  cannot be confused with earlier instances that reused request numbers.
- Collection is bounded and cannot block inference on a slow observer.

### Resource integrity

- GPU-dependent sampling remains behind the inference abstraction and optional
  CUDA boundary. CPU fixtures report unavailable GPU data.
- Device-wide usage has a stable device identity and is distinguished from
  process/model-attributable usage. Unmeasurable attribution is unavailable,
  never a fabricated zero or a model-file-size estimate.
- Observation has measurable overhead, reported separately from formal Phase 1
  end-to-end latency and accuracy acceptance.

## Constraints and non-goals

- No browser UI, persistent history, automatic repository discovery, or remote
  telemetry export; these diagnostics only supply later console tasks.
- No retrieval tuning, ranking changes, inference backend replacement, or
  completion of another task's acceptance criteria.
- Existing CPU-only validation, socket permissions, worker concurrency, and
  thin-client dependency boundaries remain intact.

## Acceptance criteria

### Agent-verifiable

1. A short-timeout daemon exits with observers attached; observer rejection,
   disconnect, and repeated reads never extend the timeout.
2. Slow startup and stale-store fixtures remain observable; observers cannot
   execute any worker operation and version mismatches are typed.
3. Two clients reusing request numbers yield distinct events; failure,
   disconnect, buffer wrap, restart, and indexing progress have regression tests.
4. Seeded sensitive input is absent from serialized diagnostic events; buffer
   size and response frame limits are enforced.
5. Search records retain their locked serialization and ordering; CPU resource
   fields are unavailable and thin-client dependency checks pass.
6. The full project validation suite passes.

### Human-verifiable

1. On the RTX 2060 with a desktop session attached, measured device identity and
   memory agree with the system device monitor; unknown attribution remains
   explicitly unavailable. Use the resource-observation procedure in the plan.
2. Compare identical real-model workloads with recording disabled and enabled
   over three paired runs, recording individual latency and memory results and
   medians using the plan's measurement procedure. Record the overhead without
   treating it as Phase 1 acceptance.
