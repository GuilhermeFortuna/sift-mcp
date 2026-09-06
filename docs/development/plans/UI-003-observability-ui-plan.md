# UI-003 implementation plan: Observability UI

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/UI-003-observability-ui-spec.md`](../specs/UI-003-observability-ui-spec.md)  
**Depends on:** UI-002

## Current-system context

UI-002 supplies the planned React/TypeScript/Vite foundation and typed HTTP
client, `/api/v1` status/activity/metrics endpoints, and SSE invalidations.
UI-001 supplies daemon lifecycle and safe metadata; do not derive activity from
browser requests. The existing `SearchDiagnostics::stage_millis` measures stages
in milliseconds while request elapsed time is expressed in microseconds.

No product screens exist before this series. Reuse the official shadcn registry
for controls instead of inventing equivalent primitives. During planning, its
MCP search returned `sidebar` and `sidebar-01`; official Vite installation and
Base UI support were verified. Inspect current registry items and licenses at
implementation, because copied component sources become maintained local code.

## Interfaces produced

Place shared shell/components in `ui/src/components`, route views in
`ui/src/pages`, and server state in `ui/src/api`. Use React Router routes `/`,
`/activity`, and later `/repositories` and `/search`. URL state may contain only
repository IDs, time range, operation, outcome, and pagination—not search text.

```typescript
// ui/src/api/hooks.ts; declarations only
import type { UseQueryResult } from '@tanstack/react-query';
import type { ActivityPage, MetricsResponse } from './types';
export type ActivityFilter = {
  repositoryId?: string;
  from: number;
  to: number;
  operation?: string;
  outcome?: string;
  cursor?: string;
};
export declare function useConsoleEvents(): void;
export declare function useActivity(filters: ActivityFilter): UseQueryResult<ActivityPage, Error>;
export declare function useMetrics(repositoryId: string | undefined,
  from: number, to: number): UseQueryResult<MetricsResponse, Error>;

// ui/src/components/AppShell.tsx
export declare function AppShell(): React.JSX.Element;
// ui/src/pages/OverviewPage.tsx
export declare function OverviewPage(): React.JSX.Element;
// ui/src/pages/ActivityPage.tsx
export declare function ActivityPage(): React.JSX.Element;
```

Shared primitives own status badges, async-state panels, metric formatting,
chart tooltips, paginated tables, and inspection drawers. Data hooks use UI-002
DTOs and query keys scoped by repository, range, and filters. Define design and
behavior ownership in root `DESIGN.md` and `UX-CONTRACT.md` before screen code.

## Implementation decisions

- Use shadcn/Base UI, Tailwind, Lucide, React Router, TanStack Query, and Recharts
  because they match the approved stack and cover accessible interaction and
  server-state concerns without a JavaScript backend. Pin versions in the lock;
  preserve upstream notices and record components/source/license in
  `ui/COMPONENTS.md`. Do not mix Base UI and Radix variants opportunistically.
- Load frontend-design and frontend-design-premium for the dashboard implementation.
  Record a compact sidebar, system sans-serif text, monospace data/code, neutral
  light/dark surfaces, and teal interaction accents in shared semantic tokens.
  Status colors always have text/icons. Make the request-stage timing strip the
  visual signature, because it directly explains Sift's work rather than adding
  decoration. No promotional hero or animated background.
- Navigation uses an all-repository selector for Overview/Activity; one repository
  for later operations. Time ranges are 15 minutes, 1 hour (default), 24 hours,
  and 7 days. Activity pages use 50 rows; the backend owns percentile/rate math
  so screens cannot compute incompatible definitions.
- Establish one EventSource connection per app shell. Invalidate affected query
  keys and refresh all visible snapshots on connection/reconnection/reset. Batch
  invalidations for 250 ms because event bursts should not flood the API. Show a
  disconnect banner; preserve last data with timestamps and no false live state.
- Freeze the current Activity page while new rows arrive; show a keyboard-usable
  New activity button to refresh, because moving rows disrupts reading. Filters
  reset pagination. Cancel or ignore older queries when selection changes.
- Use server device IDs to render a single latest device sample per device, not
  a sum. Missing identity cannot be deduplicated confidently: show per-repository
  unavailable attribution rather than an aggregate. Stale samples remain labeled.
- Persist only theme override (`system`, `light`, `dark`) and sidebar preference
  in localStorage. Keep query caches in memory, because later source content must
  not accidentally acquire a persisted cache through global configuration.
- Render null as Unavailable and real zero as zero; no-data charts have an empty
  state and gaps remain gaps. Chart summaries provide tabular/text equivalents.
  One narrow viewport uses a drawer sidebar and stacked cards; tables scroll
  inside their own labeled region. Keyboard focus and reduced motion are required.

## Ordered implementation

1. Confirm UI-002, read the spec, create `UI-003-observability-ui`, and load the
   frontend-design skills. Establish tokens, canonical component ownership, and
   component provenance before implementing views.
2. Write failing tests for theme resolution (system default, explicit override),
   null versus zero formatting, status text, and keyboard sidebar navigation.
   Run and confirm they fail. Install verified registry components and implement
   the shared shell/primitives, confirm they pass, and commit.
3. Write fixtures for two repositories, shared GPU UUID, zero requests, missing
   samples, all lifecycle states, and stale timestamps. Assert shared device usage
   is shown once and never summed. Run and confirm they fail. Implement Overview
   with real API hooks/Recharts and accessible summaries, pass, and commit.
4. Write Activity tests for filter/page URLs, safe event fields, stage units, and
   pending new rows without moving the inspected row. Run and confirm they fail.
   Implement paginated Activity and metadata-only detail drawer, pass, and commit.
5. Write reconnect/race tests: delayed repository A cannot overwrite B, reset
   fetches a new snapshot, failed SSE shows disconnected state and retains stale
   data, and recovery clears the banner. Run and confirm they fail. Implement
   shared SSE/query invalidation and async states, pass, and commit.
6. Add Playwright flows for both routes, keyboard-only filters/drawer close,
   light/dark, reduced motion, a 390px viewport, empty/loading/error states, and
   local table overflow without page overflow. Confirm they fail before closing
   the corresponding UI gaps, pass, and commit. Add these to existing `./ci.sh`
   browser checks; retain one-worker browser concurrency.
7. Human step: launch the UI using the procedure below and assess glanceable
   health, visible staleness, errors, and chart/table legibility in both themes.
   Record owner feedback without marking aesthetic acceptance yourself.
8. Run `./ci.sh`, report browser/static evidence and remaining human criterion,
   and stop before UI-004.

## Validation

Vitest covers format/state contracts; Playwright covers actual routes, selection,
SSE recovery, keyboard, viewport, and theme behavior. Use UI-002 fixture servers
or mock daemons rather than production repositories. Run the frontend-design
static audit as an additional implementation check; it does not replace browser
verification or the project validation command.

```bash
./ci.sh
pnpm --dir ui build
cargo run -p console --bin sift-console -- --listen 127.0.0.1:7331 --assets ui/dist
```

Open `http://127.0.0.1:7331/` and `/activity` for the human procedure. Until
UI-004's forms exist, use the UI-002 integration fixture or API registrations;
do not invent a second temporary registration UI.

## Handoff

Report routes, reused component provenance, tokens, actual browser viewports and
themes tested, telemetry units/gap rendering, and owner visual feedback or its
absence. Link evidence and keep task status exclusively in the ledger.
