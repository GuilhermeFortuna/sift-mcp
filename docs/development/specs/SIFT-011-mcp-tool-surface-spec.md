# SIFT-011: MCP tool surface

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Project direction:** [`../../cuda-mcp-rtx2060-plan.md`](../../cuda-mcp-rtx2060-plan.md), [`../../tech-stack.md`](../../tech-stack.md)  
**Depends on:** SIFT-010  
**Implementation plan:** [`../plans/SIFT-011-mcp-tool-surface-plan.md`](../plans/SIFT-011-mcp-tool-surface-plan.md)

## Purpose

No coding agent can reach any of this yet. The connecting piece has an unusual
constraint: it is spawned per session, so it must start in milliseconds, which
means it can hold nothing expensive and must be a thin pass-through to the
daemon. It also carries the part of the system with the least code and the most
leverage — the tool descriptions. Whether an agent calls this or falls back to
grep is decided almost entirely by that text, and the project direction is
explicit that it must be treated as a versioned, evaluated artifact rather than
a docstring.

## Requirements

### Thinness and startup

- The client process holds no model, opens no index, and performs no work at
  start beyond what the protocol handshake requires.
- Cold start to a completed handshake is within the project direction's budget,
  measured on the target machine.
- The client depends on nothing that requires a GPU, a CUDA toolkit, or an
  inference runtime, and this is enforced structurally rather than by
  convention.
- If the daemon is not running, the client causes it to start and reports
  progress rather than failing, and the request that triggered the start
  eventually succeeds.
- If the daemon cannot be started or is unreachable, the failure is reported as
  an actionable message naming the cause, not as a generic transport error.

### Tools

- The surface is exactly the Phase 1 tools named in the project direction:
  indexing a repository, searching code, finding similar code, retrieving a
  symbol's body, and nothing else.
- Each tool's parameters are typed and validated, with stated defaults and
  bounds, and an out-of-range value is rejected with a message naming the bound.
- Searching returns the triage-oriented records from SIFT-009 and never a whole
  file; retrieving a symbol is the only way to obtain a full body.
- Finding similar code accepts a code snippet rather than a query string, since
  no keyword baseline exists for it — the project direction identifies it as the
  highest-value tool for that reason.
- Retrieving a symbol identifies it by file and symbol name, and an ambiguous
  or absent symbol produces a distinguishable, actionable error.
- Indexing reports progress and completion rather than blocking silently for
  minutes.
- Errors reaching the agent state what failed and what the agent might do about
  it; an agent that receives an opaque failure falls back to grep permanently.

### Tool descriptions as an artifact

- Each tool's description states what it does, when to prefer it over the
  agent's existing tools, and carries at least one concrete example query.
- Descriptions live where they can be reviewed and changed independently of the
  code that implements the tool, and every change to them is visible in review.
- Descriptions are versioned, so a change in agent behaviour can be attributed
  to a specific description revision.
- The description text is covered by a test that fails if a tool ships without
  the required elements.

## Constraints and non-goals

- No retrieval logic, no ranking, no fusion, no caching of results. Every
  request is delegated. A client that caches becomes a client that serves stale
  results after a reindex.
- No embedding, no model loading, no GPU access of any kind.
- No tools beyond the four. Impact analysis and test selection are named in the
  project direction as Phase 3 and are deliberately not defined here; adding
  them now would ship two tools with no evaluation behind them.
- No resource or prompt surfaces beyond the tools.
- No transport other than the standard input and output stream the agent
  spawns.
- No agent A/B testing or description tuning. Measuring whether descriptions
  cause the agent to call the tools is a separate benchmark that the project
  direction keeps distinct from retrieval quality; it needs the harness from
  SIFT-012 and is judged at SIFT-013.

## Acceptance criteria

### Agent-verifiable

1. The client crate's dependency graph is asserted to contain no inference,
   GPU, or model dependency; the assertion fails if one is introduced.
2. Against a daemon backed by a mock embedder, each of the four tools is invoked
   over the real transport and returns a response matching a committed snapshot.
3. Parameter validation rejects out-of-range and wrong-typed values with a
   message naming the bound, for every tool.
4. Searching never returns a whole file or a body beyond the preview bound,
   asserted on a fixture whose files are large.
5. Retrieving an absent symbol and an ambiguous symbol each produce a
   distinguishable, actionable error.
6. With no daemon running, an invocation starts one and completes successfully.
7. With the daemon unreachable and unstartable, the error names the cause.
8. A test asserts every tool description contains a statement of when to prefer
   it over the agent's existing tools and at least one example query, and fails
   if a tool is added without them.
9. Description text carries a version identifier that changes when the text
   changes.
10. The full validation suite passes: `./ci.sh`

### Human-verifiable

1. Cold start to completed handshake is measured over at least 20 runs with the
   daemon already warm, and median and worst case are reported against the
   200 ms budget.  
   Command: `scripts/time-cold-start.sh`
2. The server is configured into a real coding agent and each of the four tools
   is exercised end to end against a real repository, confirming responses are
   usable as presented.  
   Command: `scripts/register-mcp-server.sh && <agent> --prompt "search this repo for where timestamps are normalized"`
3. Tool descriptions are read as an agent would read them and judged for whether
   they make the preference over grep unambiguous.  
   Command: `cargo run -p mcp-client -- --print-tool-descriptions`
