# UI-005: Search lab UI and console packaging

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Project direction:** [`../../tech-stack.md`](../../tech-stack.md), [`../../cuda-mcp-rtx2060-plan.md`](../../cuda-mcp-rtx2060-plan.md)  
**Depends on:** UI-004  
**Implementation plan:** [`../plans/UI-005-search-lab-ui-plan.md`](../plans/UI-005-search-lab-ui-plan.md)

## Purpose

The console explains service behavior and index maintenance but cannot yet show
why a code query returned its results. Add direct query and pasted-code search
with symbol inspection, then package and validate the complete local console.

## Requirements

### Search and inspection

- Users select one registered repository and explicitly submit a natural-language
  query or pasted-code similarity request. The result limit defaults to five and
  accepts one through twenty.
- Results preserve daemon ordering, previews, signatures, documentation, line
  ranges, and lexical/dense/fused scores. Missing scores are not zero scores.
- Available stage timings and degraded retriever state are visible without
  claiming that a fused score is a probability or confidence percentage.
- Selecting a result retrieves its indexed symbol body into an inspector.
  Missing or ambiguous symbols have explicit recovery states.
- Superseded search or symbol responses cannot replace newer selections.
  Switching repositories clears results and does not submit automatically.
- Query text, pasted code, and symbol bodies remain transient: no URL encoding,
  local persistence, browser history payload, or telemetry retention.

### Distribution and acceptance

- A distribution contains the Rust service and frontend assets and runs without
  Node.js, Python, CUDA, or a model merely to open the console. Explicit real
  inference operations still require the configured daemon and its dependencies.
- Missing frontend assets produce an actionable startup error. Ordinary Rust
  builds remain independent of generating those assets.
- The complete console covers offline/empty/error recovery, indexing initiated
  by an agent, two repositories, keyboard navigation, narrow layouts, and themes.
- A reproducible three-pair workload comparison reports collection overhead,
  including individual daemon-side latency, process RSS, available VRAM, and
  medians. Device-wide memory is not falsely attributed to one daemon.

## Constraints and non-goals

- No reranking controls, ranking tuning, code editor, code execution, file
  modification, cross-repository search, or formal Phase 1 acceptance claim.
- No remote deployment, accounts, installers for other platforms, or automatic
  daemon launch merely to display the console.
- Existing MCP result records, exclusion policies, and CPU build remain intact.

## Acceptance criteria

### Agent-verifiable

1. Query and snippet modes submit only explicitly, enforce result limits, and
   preserve daemon result order, scores, previews, and diagnostics.
2. Slow earlier responses, repository switches, missing/ambiguous symbols,
   retriever degradation, and daemon failures have regression coverage.
3. Seeded query/code content appears only in intended transient requests and
   views, never URLs, persistent browser storage, logs, or history databases.
4. The packaged console serves assets and direct routes without a JavaScript
   server; traversal and missing-asset cases are tested.
5. A complete two-repository browser workflow passes on CPU fixtures in both
   themes and a narrow viewport with accessible keyboard interactions.
6. The full project validation suite passes.

### Human-verifiable

1. Run the packaged-console procedure in the plan against the real RTX 2060
   daemon and confirm that query/snippet inspection helps explain actual agent
   results, including model/resource attribution and index freshness.
2. Execute the plan's three paired workload runs with collection disabled and
   enabled. Record every latency/RSS/VRAM result and median separately from
   SIFT-013 acceptance; do not substitute CPU fixtures for this evidence.
