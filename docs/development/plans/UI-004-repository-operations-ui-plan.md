# UI-004 implementation plan: Repository operations UI

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/UI-004-repository-operations-ui-spec.md`](../specs/UI-004-repository-operations-ui-spec.md)  
**Depends on:** UI-003

## Current-system context

UI-002 defines registration CRUD, status/freshness, start/index/jobs routes and
owns the worker connections. UI-003 supplies shared navigation, dialogs, form
feedback, query keys, theme, and event invalidation. Reuse those contracts rather
than adding a second collector or initiating indexing from an SSE handler.

The daemon's existing `IndexReportWire` has files_seen/indexed/excluded/
unsupported/unparsed, added/reused/removed chunks, embeddings, truncation, stage
durations, wall time, and before/after counts. Present these recorded facts;
neither the wire report nor matching HEAD proves that a dirty working tree is
fully reflected in the current index.

## Interfaces produced

Add routes `/repositories`, `/repositories/new`, `/repositories/:id`, and
`/repositories/:id/edit`. Reuse UI-002 registration DTOs and action endpoints.

```typescript
// ui/src/pages/RepositoriesPage.tsx; declarations only
export declare function RepositoriesPage(): React.JSX.Element;
// ui/src/pages/RepositoryDetailPage.tsx
export declare function RepositoryDetailPage(): React.JSX.Element;
// ui/src/components/RepositoryForm.tsx
export type RepositoryFormValues = {
  name: string;
  repo_path: string;
  store_path: string;
  model_path: string;
  daemon_path: string;
};
export declare function RepositoryForm(props: {
  initialValues?: RepositoryFormValues;
  onSave: (values: RepositoryFormValues) => Promise<void>;
}): React.JSX.Element;
// ui/src/components/IndexProgress.tsx
export declare function IndexProgress(props: {
  phase: string;
  done: number;
  total: number | null;
}): React.JSX.Element;
```

Use API response error codes for form mappings and busy states, not string
matching against daemon messages. Reconcile external progress through status
instance/connection/request identity; console jobs additionally carry their
service job ID. Do not fabricate a console job ID for an external operation.

## Implementation decisions

- Forms use the same five saved configuration values plus display name, because
  onboarding must be possible entirely through the UI. Path inputs are explicit
  text fields; no broad filesystem enumeration. Preserve values on server error,
  focus the first invalid field, and disable duplicate submission while pending.
- Detail view groups Identity, Index freshness, Current operation, and Last report.
  Render indexed commit, HEAD and dirty state independently. Use labels such as
  Commit aligned, Different commit, Working tree changed, and Unknown; never
  infer Fully up to date from commit equality.
- Use TanStack Query mutations for explicit Start, Update, Full rebuild, and
  registration writes. Disable automatic mutation retries, because a dropped
  response is not proof the server rejected work. Invalidate status/jobs after
  outcomes, and retain unknown/running state while reconnecting.
- Update sends mode `update`; Full rebuild sends `full` only after an app-owned
  confirmation dialog describes its cost. Removal confirmation explicitly says
  project/index files remain. Refuse registration edit/removal during owned jobs
  according to the backend's 409 contract, including races after form opening.
- Use the service job lifecycle; never tie it to route component lifetime.
  After refresh load status and jobs first, then follow SSE. Render external
  agent progress separately from console job history to avoid duplicate rows.
- Progress uses counts and phase, with a determinate bar only when total is
  positive and known. Unknown totals stay indeterminate. Final success requires
  a terminal success/report, not 100% progress or a closed stream.
- Display error-category-specific recovery text: check paths/model for startup
  failure, update binaries for incompatibility, refresh status for busy state,
  and inspect/rebuild explicitly for staleness. Never silently change paths,
  download models, kill a process, or full-rebuild to clear an error.

## Ordered implementation

1. Confirm UI-003, read the spec and shared design/UX contracts, and create
   `UI-004-repository-operations-ui`.
2. Write form tests for missing fields, server duplicate-store rejection,
   preserved input, first-invalid focus, and double-click producing one request.
   Run and confirm they fail. Implement list/new/edit routes and forms with
   shared primitives, confirm they pass, and commit.
3. Write details tests for clean matching HEAD, dirty matching HEAD, different
   HEAD, unborn repository, and inspection failure. Assert none claims working
   tree freshness from commit equality. Run and confirm they fail. Implement
   details and report sections, pass, and commit.
4. Write browser tests proving Update sends update, cancelled rebuild sends
   nothing, confirmed rebuild sends full once, and busy/start/mismatch states
   are actionable. Run and confirm they fail. Implement mutations/dialogs,
   confirm they pass, and commit.
5. Write two-repository tests: refresh during a slow index, switch repository,
   reconnect SSE, agent-initiated indexing, unknown total, failed terminal event,
   and registration edit/removal racing with a new owned job. Run and confirm
   they fail. Implement progress/reconciliation, pass, and commit. Include a
   filesystem-backed API test proving removal left project/index files intact.
6. Add keyboard, light/dark, and 390px browser flows for registration, confirmation,
   and progress. Confirm they fail for missing behavior, fix and pass, then commit.
7. Human step: launch using the procedure below, register two real repositories,
   explicitly run an incremental update, inspect Full rebuild confirmation
   without running an unnecessary rebuild, and initiate an index through an
   existing MCP agent to verify external progress. Record usability feedback.
8. Run `./ci.sh`, report exactly which real operations were performed versus
   fixtures, preserve pending human acceptance, and stop before UI-005.

## Validation

Vitest verifies formatting/forms; Playwright exercises real routes and dialogs;
CPU daemon/API tests prove background jobs and removal safety. No human testing
implicitly authorizes indexing a repository: the owner chooses registered paths
and clicks the explicit action during the procedure.

```bash
./ci.sh
pnpm --dir ui build
cargo run -p console --bin sift-console -- --listen 127.0.0.1:7331 --assets ui/dist
```

Open `http://127.0.0.1:7331/repositories` for registration and the manual flow.

## Handoff

Report registration/action tests, freshness-label evidence, external and UI job
identity handling, browser recovery checks, actual viewports/themes, and any
owner feedback still outstanding. Do not claim to have cancelled daemon work
when only a browser request was aborted.
