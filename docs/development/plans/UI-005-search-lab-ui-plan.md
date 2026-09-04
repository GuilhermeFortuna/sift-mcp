# UI-005 implementation plan: Search lab UI and console packaging

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/UI-005-search-lab-ui-spec.md`](../specs/UI-005-search-lab-ui-spec.md)  
**Depends on:** UI-004

## Current-system context

`retrieval::SearchResponse` contains ordered `SearchResult` records and
`SearchDiagnostics`; previews are limited to 320 UTF-8-safe bytes, scores are
nullable, and stage timings cover embed/lexical/dense/fuse/assemble/total.
The four existing MCP tools already provide search, similar code, symbol
retrieval, and indexing. This UI must expose those capabilities without
introducing an alternative ranking pipeline.

UI-002 supplies search/similar/symbol endpoints and static asset serving;
UI-003/004 supply the shell, async feedback, repository selector, and component
contracts. UI-001 supplies the instrumentation measurement helper. Reuse all of
these and package the resulting console for operation without a Node runtime.

## Interfaces produced

```typescript
// ui/src/pages/SearchPage.tsx; declarations only
export declare function SearchPage(): React.JSX.Element;
// ui/src/components/SearchResults.tsx
export declare function SearchResults(props: {
  response: SearchResponse;
  onSelect: (result: SearchResult) => void;
}): React.JSX.Element;
// ui/src/components/SymbolInspector.tsx
export declare function SymbolInspector(props: {
  repositoryId: string;
  result: SearchResult;
}): React.JSX.Element;
// SearchResponse/SearchResult refer to UI-002's existing DTOs, not new schemas.
```

Route `/search?repository=<id>` accepts only repository identity in the URL.
Query/code/mode/result selection stays in component memory. POST `/search`,
`/similar`, and `/symbol` use the existing scoped API and CSRF-aware client.

Add `scripts/package-console.sh --output <directory>` producing an archive with
`sift-console`, `ui/` assets, usage documentation, and third-party notices.
Add `scripts/measure-console-overhead.sh` as described below, reusing the
UI-001 Rust measurement helper rather than introducing runtime Python.

## Implementation decisions

- Use explicit Query and Similar code tabs and a labeled auto-growing textarea,
  because pasted snippets do not need a full editor/runtime. Submit via button
  or Ctrl/Cmd+Enter, respecting IME composition; do not search while typing.
  Result limit defaults to 5 and validates integer 1–20 in both client and server.
- Render results in daemon order with file/symbol/signature/doc/preview/lines and
  raw lexical/dense/fused scores. Missing scores display Unavailable, not zero;
  fused score is never a confidence percentage. Show existing stage timings and
  safe degradation flags without retaining raw retriever error strings in history.
- Use a split pane for results and symbol body on wide screens, stacked inspector
  on narrow screens. Render source as escaped text in a read-only code block;
  no HTML interpretation, editor actions, or executing content. Preserve line
  numbers and allow an explicit copy action.
- Search and symbol fetches use mutation-local memory plus AbortController and
  monotonically increasing request generation, because cancellation alone does
  not guarantee an old response cannot arrive. Reset results/selection on repository
  switch and ignore stale search and symbol responses independently.
- A clear button empties input, cancels pending display updates, resets results,
  and returns focus. Leaving the page discards transient source/query state.
  Never place source in query keys, URL state, localStorage, persisted caches,
  tracing fields, screenshots used as committed fixtures, or service history.
- Handle SymbolNotFound and SymbolAmbiguous explicitly. For ambiguity, show
  returned candidate names and retrieve only an explicitly chosen candidate;
  an index change may invalidate a prior result, so offer a new search instead
  of reading current disk contents to fill the missing symbol.
- Package external assets beside the binary, because compile-time embedding
  would couple ordinary Cargo builds to Node output. The packaging script builds
  with the frozen pnpm lock and Cargo release profile and copies assets/notices.
  Distribution contains no models and launches without inference dependencies.
- Add a `--collect off` console measurement option that disables background
  collection but preserves explicit operations. The overhead helper uses the
  same warmed daemon/model/store/query fixture with console collection off/on
  for three paired runs, rotating pair order to limit ordering bias. Never alter
  indexing or retrieval parameters between paired runs.
- Record per-run p50/p95, sample counts, daemon and console RSS separately, and
  available device/process VRAM with UUID. Compute medians across three runs and
  relative p95 change; an unavailable baseline yields unavailable ratio. This
  evaluates overhead and does not replace the pinned evaluation corpora.

## Ordered implementation

1. Confirm UI-004, read the spec/design contracts, and create
   `UI-005-search-lab-ui`.
2. Write tests for explicit submission, IME, Ctrl/Cmd+Enter, query/snippet endpoint
   selection, defaults, and limits 0/1/20/21 plus fractional values. Run and
   confirm they fail. Implement Search page inputs and scoped mutations, pass,
   and commit.
3. Write fixtures asserting order/scores/320-byte previews are unmodified, null
   differs from zero, degraded retrieval remains visible, and timing units match
   existing DTOs. Run and confirm they fail. Implement results/diagnostics using
   shared primitives, pass, and commit.
4. Write tests for delayed A after B, repository switch, clear during a request,
   delayed symbol response, missing and ambiguous symbols, and escaped source
   containing HTML/script-like text. Run and confirm they fail. Implement the
   inspector and race protection, pass, and commit.
5. Seed unique private query/code values; run browser search and inspect URLs,
   localStorage/sessionStorage, console/service logs, and SQLite history to assert
   they are absent. Confirm tests fail if any path persists content, implement
   necessary privacy boundaries, pass, and commit. Add negative fixtures to prove
   the assertions detect leaks rather than relying on an initially empty log.
6. Write package smoke tests for assets, direct `/search` navigation, missing
   assets, traversal, and a runtime PATH without Node/pnpm/Python. Confirm they
   fail. Implement packaging with notices and usage docs, pass, and commit.
7. Add CPU report tests and the overhead helper, confirming aggregation/disabled
   collection assertions fail before implementation. Add a full Playwright flow:
   register two fixtures, inspect health/activity, index, search/snippet, inspect
   symbol, recover from daemon outage, and verify keyboard, 390px, both themes.
   Run and confirm they fail before completing missing integration, pass, commit.
8. Human step: use the packaged-console procedure below with real daemon/model
   paths. Verify query/snippet results alongside an actual MCP-agent result,
   resource attribution, and freshness; record usefulness and visual feedback.
9. Human step: run three off/on collection pairs below with identical warmed
   workload, preserve every report and median, and report overhead without
   asserting Phase 1 acceptance. Leave this pending without owner evidence.
10. Run `./ci.sh`, inspect final package and dependency closure, report remaining
    acceptance evidence, and stop. Do not begin another task series.

## Validation

Use existing CPU fixtures for deterministic rank/record parity and real browser
tests for interaction/privacy. Packaging smoke tests must launch the actual
produced binary and assets, not Vite's development or preview server. Keep browser
workers at one and include all checks through `./ci.sh`.

```bash
./ci.sh
scripts/package-console.sh --output "$SIFT_UI_PACKAGE_DIR"
"$SIFT_UI_PACKAGE_DIR/sift-console" --listen 127.0.0.1:7331
scripts/measure-console-overhead.sh --repo "$SIFT_REPO" --store "$SIFT_STORE" --model "$SIFT_MODEL" --daemon "$SIFT_DAEMON" --console "$SIFT_UI_PACKAGE_DIR/sift-console" --runs 3 --output "$SIFT_UI_REPORT_DIR"
```

The package command also leaves the unpacked bundle in the named output directory
so the launch command is literal. Use a new output directory; refuse overwriting
an existing nonempty bundle. Open `http://127.0.0.1:7331/search` for the human flow.
The measurement helper starts/stops only its own console instances and refuses
to replace a daemon it did not launch for the dedicated measurement store.

## Handoff

Report the completed console routes, package location and no-Node runtime test,
privacy/race/ranking regressions, full validation outcome, and owner evidence for
real-agent usefulness and all three paired measurements. Include raw per-run
figures and medians or list them as outstanding; update only the ledger for task
status and never claim the UI establishes SIFT-013 acceptance.
