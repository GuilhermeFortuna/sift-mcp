# UI-003: Observability UI

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Project direction:** [`../../tech-stack.md`](../../tech-stack.md), [`../../cuda-mcp-rtx2060-plan.md`](../../cuda-mcp-rtx2060-plan.md)  
**Depends on:** UI-002  
**Implementation plan:** [`../plans/UI-003-observability-ui-plan.md`](../plans/UI-003-observability-ui-plan.md)

## Purpose

The console service supplies status and history, but an operator still needs to
interpret raw diagnostics. Create a shared application shell and Overview and
Activity views that explain whether Sift is working, which repository is
affected, and where request time is spent.

## Requirements

### Overview and activity

- A compact navigation shell supports an all-repository overview and explicit
  repository selection. Direct navigation and browser back/forward preserve
  selected repository, time range, and activity filters.
- Overview shows daemon state, model identity, live/dead chunks, indexed
  commit, recent errors, request rate, latency, and available GPU measurements.
- Activity shows daemon-observed worker requests, including MCP-agent requests,
  with time, repository, operation, duration, outcome, result count, and available
  stage timings. No sensitive request content is fetched for history display.
- Charts expose units, time window, sample count, and coverage gaps. Device-wide
  memory is grouped by device identity, never summed across repositories.
- Latency is labeled daemon-side. Formal acceptance thresholds are not displayed
  as achieved based on these observations.

### Shared interaction quality

- System light/dark preference is the default, with a persisted user override.
  Status is understandable without relying solely on color.
- Shared accessible controls, tokens, tables, charts, feedback, and navigation
  are reused across screens and documented with component provenance.
- Loading, empty, error, disconnected, stale-data, and unsupported-measurement
  states explain what happened and which action is available.
- Keyboard navigation, visible focus, reduced motion, narrow layouts, and
  accessible chart summaries work. Dense tables remain readable without making
  the entire page scroll horizontally.
- Live activity never steals focus or unexpectedly moves the row being read.

## Constraints and non-goals

- No repository editing, indexing controls, search screen, alert delivery,
  custom dashboards, or ranking controls in this task.
- No new polling path that starts daemons or changes their residency.
- English interface for one local user; application preference persistence
  contains only non-sensitive display settings.

## Acceptance criteria

### Agent-verifiable

1. Overview and Activity render two-repository fixtures, unavailable values,
   zero-valued measurements, gaps, empty windows, and every lifecycle state.
2. Filtering and navigation work without mixing repositories; event reconnects
   refresh data and a slow earlier response cannot replace a newer selection.
3. Shared-device fixtures render one device reading rather than a sum; latency
   and rate labels match backend units and coverage.
4. Browser tests exercise keyboard controls, table overflow, reduced motion,
   narrow layout, both themes, loading, failure, and disconnected recovery.
5. The full project validation suite passes.

### Human-verifiable

1. With the console launched using the plan's preview procedure, verify that
   service health, stale observations, and recent failures are recognizable at
   a glance, and that charts and tables remain legible in both themes.
