# SIFT-010: Resident daemon

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Project direction:** [`../../cuda-mcp-rtx2060-plan.md`](../../cuda-mcp-rtx2060-plan.md), [`../../tech-stack.md`](../../tech-stack.md)  
**Depends on:** SIFT-006, SIFT-009  
**Implementation plan:** [`../plans/SIFT-010-resident-daemon-plan.md`](../plans/SIFT-010-resident-daemon-plan.md)

## Purpose

Everything built so far runs as a one-shot process, and an MCP server is spawned
per client and often per session. Paying model load and index open on every
agent start is tens of seconds against a 400 ms request budget, which the
project direction identifies as fatal. This task makes the expensive state
resident in a long-lived process reachable over a local socket, so that the
process an agent actually launches holds nothing expensive at all. It also owns
the two policies that only a resident process can have: how concurrent requests
share one GPU, and when to give the VRAM back.

## Requirements

### Residency and lifetime

- The embedding model, the chunk store, and both retrieval indexes are loaded
  once at daemon start and reused across requests and across clients.
- The daemon starts automatically on the first connection attempt if it is not
  running, and a client that starts it does not fail the request that triggered
  the start.
- Exactly one daemon runs per store; a second start attempt for the same store
  defers to the running one rather than competing for it.
- The daemon exits after a configured idle period with no connected clients,
  releasing GPU memory, and a subsequent request starts it again.
- A daemon whose store has been deleted or replaced underneath it detects this
  and refuses to serve stale results.
- Shutdown, whether idle or signalled, completes in-flight requests and leaves
  the store verifiable.

### Transport and protocol

- Clients reach the daemon over a local socket that is not exposed to the
  network and is not readable by other users on the machine.
- The protocol carries request framing, a request identifier, and typed errors,
  so a client can distinguish "not found" from "model unavailable" from "daemon
  is starting".
- The protocol is versioned, and a client and daemon whose versions disagree
  fail with a clear message rather than misinterpreting each other's bytes.
- A malformed or oversized request is rejected without affecting other clients
  or the daemon's health.
- Multiple clients are served concurrently, and one client's slow or abandoned
  request does not block another's.

### Request handling

- The daemon serves search, similarity, symbol retrieval, and indexing requests
  by delegating to the components already built, adding no retrieval logic of
  its own.
- Indexing is long-running and does not block search requests for its duration;
  its progress is observable while it runs.
- Only one index operation runs at a time per store, because the store supports
  a single writer.
- Requests that arrive while the index is being modified either see a consistent
  view or are told to retry; they never see a half-updated index.
- Every request is logged with its type, its outcome, and its per-stage
  latencies, because the latency SLO cannot be defended without per-request
  measurement.

### Resource policy

- GPU memory held by the daemon is bounded and reported, and the bound accounts
  for the desktop session sharing the device.
- Concurrent requests do not each allocate their own inference resources; the
  policy for sharing the model across concurrent requests is stated.
- If the GPU becomes unavailable, the daemon reports a distinguishable error and
  does not silently fall back to a path that would blow the latency budget.

## Constraints and non-goals

- No MCP protocol, no stdio server, no tool definitions. SIFT-011 owns those,
  and putting MCP types in the daemon would drag the protocol into the process
  that must not restart.
- No network transport, no authentication, no multi-user access. A local socket
  with filesystem permissions is the security boundary. Adding a TCP listener
  "for convenience" is the temptation this rules out.
- No reranking and no model beyond the embedder resident. The reranker's lazy
  load and idle eviction are described in the project direction as Phase 2 work
  and arrive with the reranker.
- No filesystem watching or automatic reindexing on change. Indexing is
  requested.
- No multi-store or multi-repository serving from one daemon. One store per
  daemon.
- No request queue with priorities, no admission control, no backpressure
  tuning. A simple concurrency limit until measurement shows it is insufficient.
- No persistent request history, metrics endpoint, or trace export. Structured
  logs only.

## Acceptance criteria

### Agent-verifiable

1. With a mock embedder, a client connects, issues a search, and receives results
   equal to those the underlying components produce in-process.
2. A second daemon start attempt for the same store does not produce a second
   listener, and the second attempt's requests are served by the first daemon.
3. The socket is not reachable over the network and its permissions deny other
   users, asserted by test.
4. A client at a mismatched protocol version receives a clear version error.
5. A malformed request and an oversized request are each rejected, and a
   subsequent well-formed request on a new connection succeeds.
6. Two concurrent clients are served concurrently, asserted by a test that would
   fail under serialization.
7. A search issued during an in-progress index returns either a consistent
   result or an explicit retry signal, never a partial view.
8. A second concurrent index request is refused or serialized, never run
   alongside the first.
9. The daemon exits after the configured idle period and a subsequent request
   starts it again transparently.
10. In-flight requests complete on signalled shutdown, and the store verifies
    afterwards.
11. Every request emits a log line carrying type, outcome, and per-stage
    latencies.
12. The full validation suite passes: `./ci.sh`

### Human-verifiable

1. Daemon start time with the real model and a full-size index is measured on
   the target machine and reported, establishing the cost that residency exists
   to amortize.  
   Command: `scripts/time-daemon-start.sh <store-path>`
2. Resident GPU memory with the embedder loaded and a full-size index open is
   measured with a desktop session attached and reported against the ~5.0 GB
   budget.  
   Command: `scripts/report-daemon-vram.sh <store-path>`
3. The daemon is left running for longer than the idle period with no clients,
   and its exit and the release of GPU memory are confirmed.  
   Command: `scripts/observe-idle-eviction.sh <store-path>`
4. A repository is indexed through the daemon while searches are issued
   throughout, confirming searches continue to be served and progress is
   visible.  
   Command: `scripts/index-under-load.sh <repo-path>`
