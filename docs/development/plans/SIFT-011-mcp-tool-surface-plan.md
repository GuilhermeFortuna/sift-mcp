# SIFT-011 implementation plan: MCP tool surface

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/SIFT-011-mcp-tool-surface-spec.md`](../specs/SIFT-011-mcp-tool-surface-spec.md)  
**Depends on:** SIFT-010

## Current-system context

`crates/mcp-client` is empty from SIFT-001, which documented it as the crate
that must depend on nothing heavy. SIFT-010 put `DaemonClient` and the protocol
types in `crates/daemon::protocol` and `::client` specifically so this crate can
reuse them: `connect_or_spawn` already handles spawning a daemon and retrying
with backoff, `request_streaming` already yields `IndexProgress` frames, and
`DaemonError` is already typed with `Starting`, `IndexInProgress`,
`SymbolNotFound`, `SymbolAmbiguous`, `GpuUnavailable`, and `StoreStale`.
`retrieval::SearchResult` (SIFT-009) has a committed serialization snapshot
matching the design document's response shape.

`rmcp` is named in `docs/tech-stack.md` as the official Rust MCP SDK and is not
yet a workspace dependency. The gap this task closes is that no coding agent can
reach any of this, and the tool descriptions — which the design document calls a
tuned artifact that decides whether the agent calls the tool at all — do not
exist.

## Interfaces produced

```rust
// crates/mcp-client/src/tools.rs
/// Bumped whenever any description text changes, so a behaviour change in an
/// agent can be attributed to a revision. Asserted by test to match the file.
pub const DESCRIPTIONS_VERSION: u32 = 1;

/// Loaded from descriptions.toml at compile time; see Implementation decisions.
pub struct ToolDescription {
    pub name: &'static str,
    pub summary: &'static str,
    pub prefer_over: &'static str,   // "prefer this over grep when ..."
    pub examples: &'static [&'static str],
}

pub fn descriptions() -> &'static [ToolDescription];
/// summary + prefer_over + examples, rendered into the MCP description field.
pub fn rendered(name: &str) -> String;
```

```rust
// crates/mcp-client/src/params.rs
pub struct SearchCodeParams {
    pub query: String,               // non-empty, at most QUERY_MAX_CHARS
    pub top_k: usize,                // default 5, 1..=TOP_K_MAX
}
pub struct FindSimilarCodeParams {
    pub code: String,                // non-empty, at most CODE_MAX_CHARS
    pub top_k: usize,                // default 5, 1..=TOP_K_MAX
}
pub struct GetSymbolParams {
    pub file: String,                // repository-relative
    pub symbol: String,              // qualified as SIFT-003 emits, e.g. Type::method
}
pub struct IndexRepositoryParams {
    pub path: String,
    pub full: bool,                  // default false: incremental update
}

pub const TOP_K_MAX: usize = 20;
pub const QUERY_MAX_CHARS: usize = 1000;
pub const CODE_MAX_CHARS: usize = 20_000;

/// Names the violated bound in the message, per the spec.
pub fn validate<T: Validate>(params: &T) -> Result<(), ParamError>;
```

```rust
// crates/mcp-client/src/server.rs
pub struct SiftMcpServer { /* store dir resolution, DaemonClient */ }

impl SiftMcpServer {
    pub fn new(store_dir: PathBuf) -> Self;
    /// Serves the MCP stdio transport until the stream closes.
    pub async fn serve_stdio(self) -> Result<(), ServeError>;
}

/// Every DaemonError becomes an agent-facing message naming cause and remedy.
fn to_tool_error(err: daemon::DaemonError) -> rmcp::Error;
```

```toml
# crates/mcp-client/descriptions.toml   (the tuned artifact, reviewed on its own)
version = 1

[search_code]
summary = "..."
prefer_over = "..."
examples = ["..."]
```

## Implementation decisions

- **Descriptions live in `descriptions.toml`, included at compile time, not in
  doc comments or attribute strings.** The design document requires them to be
  versioned and evaluated as an artifact; a single file makes a description
  change a one-file diff a reviewer can read as prose, rather than a change
  buried in a macro invocation next to code.

- **`DESCRIPTIONS_VERSION` is asserted by test to equal the `version` key in the
  file, and the file is the source of truth.** Two copies of a version number
  drift, and the whole point of the version is to attribute an agent-behaviour
  change to a specific text revision.

- **Every description carries `prefer_over` and at least one example, enforced
  by a test over the parsed table rather than by convention.** The design
  document names both as required elements; a test that iterates the table
  fails when a fifth tool is added without them, which convention would not.

- **The crate depends on `crates/daemon` for protocol types only, and
  `crates/daemon`'s heavy components are behind its own feature or are simply
  not linked because they are unreachable.** The alternative — a third protocol
  crate — is more structure than a protocol module needs. The spec's structural
  requirement is enforced by a test asserting the dependency graph, not by the
  absence of an import.

- **A build-time or test-time assertion walks `cargo metadata` for this crate
  and fails if `ort`, `cudarc`, `tokenizers`, or `tantivy` appear.** The spec
  requires this structurally rather than by convention, and a dependency added
  three tasks from now is exactly what a convention misses.

- **`connect_or_spawn` is called lazily on the first tool invocation, not during
  server construction or the MCP handshake.** The 200 ms cold-start budget is
  measured to a completed handshake; connecting to or spawning a daemon during
  startup would put a model load on the critical path of every agent session,
  which is the exact cost the daemon exists to avoid.

- **`DaemonError::Starting` is retried inside the tool call with backoff until a
  deadline, and only then surfaced.** The spec requires that the request which
  triggered a daemon start eventually succeeds; surfacing `Starting` to the
  agent would make the first search of every cold session fail, and an agent
  that gets one failure stops calling the tool.

- **Every `DaemonError` maps to a message naming the cause and the remedy —
  `GpuUnavailable` says the daemon has no GPU and names the detail,
  `IndexInProgress` says to retry shortly, `SymbolAmbiguous` lists the
  candidates, `StoreStale` says to re-index.** An opaque transport error teaches
  the agent that the tool is unreliable, and that lesson is not unlearned within
  a session.

- **`top_k` defaults to 5 and is capped at 20.** The design document's tool
  signatures specify `top_k=5`, and the cap exists because the value flows into
  fusion depth and result assembly; an unbounded `top_k` lets one request return
  the entire corpus's metadata.

- **`code` for `find_similar_code` is capped at 20,000 characters.** It is
  embedded as a document and will be truncated at the model's sequence limit
  anyway; the cap makes the truncation an explicit rejection rather than silent
  loss of most of the input.

- **`index_repository` defaults to incremental rather than full.** A full
  re-index is minutes and an agent calling the obvious-sounding default should
  not pay it; SIFT-006 makes the incremental path correct from an empty store,
  so `full` is only needed to force a rebuild.

- **`index_repository` streams progress as MCP progress notifications and
  returns the `IndexReport` summary.** The spec requires progress rather than a
  silent multi-minute block, and `request_streaming` already yields the frames.

- **`get_symbol` takes the qualified symbol name SIFT-003 emits, and the
  description says so with an example.** An agent passing a bare method name
  into a file with two same-named methods gets `SymbolAmbiguous` with
  candidates, which is recoverable — but only if the description told it the
  qualified form exists.

- **Search responses are serialized from `retrieval::SearchResult` unchanged.**
  SIFT-009 locked that shape against the design document's example, and a
  reshaping layer here would be a second place for the response format to drift.

- **The four tools are the whole surface; no resources or prompts are
  registered.** The spec rules them out, and an MCP server advertising unused
  capabilities invites the agent to probe them.

## Ordered implementation

1. Create the branch `SIFT-011-mcp-tool-surface`.
2. Declare `rmcp` in `[workspace.dependencies]` and inherit it in
   `crates/mcp-client` along with `tokio`, `serde`, and a dependency on
   `crates/daemon`. Confirm `./ci.sh` passes. Commit.
3. Write a failing test that walks `cargo metadata` for `mcp-client` and asserts
   its full dependency closure contains none of `ort`, `cudarc`, `tokenizers`,
   or `tantivy`. Run it and confirm it fails if a GPU dependency is temporarily
   added, then remove the addition and confirm it passes. Commit.
4. Write `descriptions.toml` with the four tools, each carrying `summary`,
   `prefer_over`, and at least one example. Write failing tests asserting: the
   file parses; every tool has a non-empty `prefer_over` and at least one
   example; `DESCRIPTIONS_VERSION` equals the file's `version`; the rendered
   description contains all three parts. Run and confirm they fail. Implement
   loading and rendering. Confirm they pass. Commit.
5. Write failing tests for parameter validation: `top_k` of 0 and of
   `TOP_K_MAX + 1` are rejected with a message naming the bound; an empty query
   is rejected; a query above `QUERY_MAX_CHARS` is rejected naming the limit;
   `code` above `CODE_MAX_CHARS` is rejected; omitted `top_k` defaults to 5;
   omitted `full` defaults to false; a wrong-typed field is rejected. Run and
   confirm they fail. Implement validation. Confirm they pass. Commit.
6. Write a failing integration test that starts a daemon over `MockEmbedder`
   against a fixture store, drives the MCP server over the real stdio transport,
   and asserts `search_code` returns a response matching a committed snapshot.
   Run and confirm it fails. Implement the server and the `search_code` handler.
   Confirm it passes. Commit.
7. Add the same test for `find_similar_code`, `get_symbol`, and
   `index_repository`, one per commit, each with its own committed snapshot, and
   for `index_repository` asserting progress notifications arrive before the
   summary.
8. Write a failing test asserting `search_code` never returns a whole file or a
   body beyond `PREVIEW_MAX_BYTES`, over a fixture whose files are large, and
   that `get_symbol` is the only tool returning a full body. Run and confirm it
   fails. Confirm the handlers. Confirm it passes. Commit.
9. Write failing tests for symbol errors: an absent symbol produces a message
   naming the file and symbol; an ambiguous symbol produces a message listing
   the candidates. Run and confirm they fail. Implement `to_tool_error` for
   those variants. Confirm they pass. Commit.
10. Write a failing test for cold start: with no daemon running, a `search_code`
    invocation spawns one and returns results, retrying `Starting` internally
    rather than surfacing it. Run and confirm it fails. Implement lazy
    connect-or-spawn with backoff to a deadline. Confirm it passes. Commit.
11. Write a failing test for unreachable daemon: with spawning disabled and no
    daemon present, the error message names the cause and the remedy rather than
    reporting a bare transport failure. Add tests mapping `GpuUnavailable`,
    `IndexInProgress`, and `StoreStale` to actionable messages. Run and confirm
    they fail. Complete `to_tool_error`. Confirm they pass. Commit.
12. Write a failing test asserting the advertised capability set contains
    exactly the four tools and no resources or prompts. Run and confirm it
    fails. Confirm registration. Confirm it passes. Commit.
13. Add the `--print-tool-descriptions` flag rendering every description as the
    agent would receive it. Add `scripts/time-cold-start.sh` measuring spawn to
    completed handshake over N runs with the daemon already warm, and
    `scripts/register-mcp-server.sh` emitting the configuration snippet for a
    coding agent. Commit.
14. Human step: run `scripts/time-cold-start.sh` over at least 20 runs with the
    daemon warm and report median and worst case against the 200 ms budget.
15. Human step: register the server with a real coding agent via
    `scripts/register-mcp-server.sh`, exercise all four tools against a real
    repository, and report whether each response was usable as presented.
16. Human step: run `cargo run -p mcp-client -- --print-tool-descriptions`, read
    them as an agent would, and judge whether the preference over grep is
    unambiguous.
17. Run the full validation suite and confirm it passes.

## Validation

- **Unit:** dependency-closure assertion; description completeness and version
  agreement; parameter validation across bounds, defaults, and wrong types.
- **Integration:** each of the four tools over the real stdio transport against
  a daemon backed by `MockEmbedder`, snapshot-compared; progress notifications
  before the index summary; preview bound enforcement; symbol error mapping;
  cold start with no daemon; unreachable-daemon and typed-error messages;
  advertised capability set.
- **Regression:** the four response snapshots are the locked reference, and they
  must remain consistent with SIFT-009's `SearchResult` snapshot — a divergence
  between them means the pass-through gained a transformation it should not
  have.
- **Manual:** exercising all four tools from a real coding agent against a real
  repository, and reading the descriptions as the agent receives them; correct
  means responses are usable without follow-up file reads for triage, and the
  grep preference is unambiguous.
- **Measurement:** cold start to completed handshake over at least 20 runs with
  the daemon warm, median and worst case against the 200 ms budget; the same
  with the daemon cold, reported separately so the daemon spawn cost is visible.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
./ci.sh
scripts/time-cold-start.sh
cargo run -p mcp-client -- --print-tool-descriptions
scripts/register-mcp-server.sh
```

## Handoff

Report median and worst-case cold start to completed handshake over at least 20
runs with the daemon warm, and the same with the daemon cold, against the 200 ms
budget; confirmation from the dependency-closure test that no inference, GPU,
tokenizer, or search-index dependency reaches this crate; the four tools
registered and the parameter bounds enforced for each; `DESCRIPTIONS_VERSION` as
shipped and the full rendered text of each description; the outcome of
exercising all four tools from a real coding agent, naming any response that
required a follow-up file read to triage; the messages produced for absent
symbol, ambiguous symbol, unreachable daemon, and GPU unavailable; and
confirmation that a cold invocation with no daemon running spawned one and
succeeded without surfacing a retry to the agent.
