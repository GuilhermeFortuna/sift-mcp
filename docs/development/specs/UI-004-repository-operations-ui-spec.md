# UI-004: Repository operations UI

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Project direction:** [`../../tech-stack.md`](../../tech-stack.md), [`../../cuda-mcp-rtx2060-plan.md`](../../cuda-mcp-rtx2060-plan.md)  
**Depends on:** UI-003  
**Implementation plan:** [`../plans/UI-004-repository-operations-ui-plan.md`](../plans/UI-004-repository-operations-ui-plan.md)

## Purpose

Operators need to attach repositories and maintain their indexes without
constructing MCP calls. Provide explicit registration, start, and indexing
workflows that show freshness honestly and retain operation progress across
browser navigation.

## Requirements

### Registration and inspection

- Users can add, edit, and remove registrations through labeled forms with
  inline errors and preserved input. Removal explains that files and indexes
  remain intact and requires explicit confirmation.
- Repository details show configured locations, model, daemon state, indexed
  commit, current HEAD, working-tree state, live/dead chunks, latest indexing
  report, and exclusion counts from that report.
- Matching HEAD is described as commit alignment, not proof of working-tree
  freshness. Dirty, unborn, unreadable, and unavailable Git states are distinct.
- Historical reports are labeled by time and indexed commit; exclusion summaries
  never imply that excluded sensitive files were read or their contents retained.

### Operations

- Start, incremental Update, and Full rebuild are explicit actions. Rebuild
  explains its cost and requires confirmation; incremental update is the default.
- Repeated clicks cannot create duplicate jobs. Busy, startup, incompatibility,
  stale-store, missing-model, and operation-failure states provide actionable
  feedback without automatic destructive recovery.
- Progress includes phase, completed count, optional total, and final outcome.
  Unknown totals use indeterminate presentation rather than invented percentages.
- Operations initiated by an MCP agent appear alongside UI-initiated operations.
  Refreshing or switching repositories does not cancel indexing or mix progress.
- Registration changes are refused while a console-owned job uses that
  registration; disconnecting the browser never claims to stop daemon work.

## Constraints and non-goals

- No index deletion, forced shutdown, automatic scheduling, model download,
  recursive filesystem picker, or editing other agents' MCP registrations.
- No search screen or changes to indexing/exclusion semantics.
- Reuse the shared shell, forms, dialogs, and live-data contracts from UI-003.

## Acceptance criteria

### Agent-verifiable

1. Browser tests add/edit/remove registrations; duplicate stores and invalid
   paths produce inline errors without losing entered values.
2. Removal leaves files and stores untouched; pending-job registration edits
   are refused with a clear busy state.
3. Clean matching HEAD, dirty matching HEAD, different HEAD, unborn, and failed
   inspection states never overclaim freshness.
4. Incremental and full workflows use the intended mode; rebuild confirmation,
   duplicate-click protection, concurrent-job rejection, reconnect, and failure
   have browser coverage.
5. MCP-initiated progress appears; unknown totals remain indeterminate and two
   repositories never share the wrong job or report.
6. The full project validation suite passes.

### Human-verifiable

1. Using the plan's console launch procedure with two real registrations,
   verify that incremental update, rebuild confirmation, external-agent progress,
   and freshness explanations are understandable without reading protocol docs.
