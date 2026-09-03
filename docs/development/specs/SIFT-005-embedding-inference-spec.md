# SIFT-005: Embedding inference

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Project direction:** [`../../cuda-mcp-rtx2060-plan.md`](../../cuda-mcp-rtx2060-plan.md), [`../../tech-stack.md`](../../tech-stack.md)  
**Depends on:** SIFT-004  
**Implementation plan:** [`../plans/SIFT-005-embedding-inference-plan.md`](../plans/SIFT-005-embedding-inference-plan.md)

## Purpose

SIFT-004 produced an exported graph and a fixture of known-correct vectors, but
nothing in the runtime can yet turn text into an embedding. The two places this
goes wrong are numerical — pooling and normalization reimplemented in Rust that
disagree with the reference by enough to degrade ranking without breaking
anything — and structural: if every crate that needs an embedding depends
directly on the GPU runtime, the workspace stops building on machines without
one and the evaluation harness cannot be tested at all. This task provides
batched embedding behind an abstraction, proven against the fixture, with a
mock that lets everything downstream be tested on CPU.

## Requirements

### Correctness

- Embedding the fixture's inputs reproduces the fixture's vectors within the
  tolerance the fixture states, and a run that exceeds the tolerance fails
  rather than warning.
- Tokenization matches the fixture's pinned token sequences exactly, so a
  divergence is attributed to tokenization or to the model, never ambiguously.
- Pooling and normalization follow the strategy the exported model records,
  rather than a strategy chosen at the call site.
- Padding in a batch does not affect any sequence's vector: a string embedded
  alone and the same string embedded alongside a much longer one yield the same
  vector within tolerance.
- Inputs longer than the model's maximum sequence length are truncated by a
  stated rule, and truncation is observable to the caller rather than silent.
- Queries and documents are embedded through the same code path, differing only
  by the instruction prefix convention the model records, so an asymmetry cannot
  be introduced by accident.

### Abstraction and testability

- Every consumer depends on an abstraction over embedding, not on the GPU
  runtime, and the workspace builds and tests without any GPU dependency
  present.
- A deterministic mock satisfies the same abstraction and produces stable
  vectors for the same input, so downstream ranking logic is testable without
  hardware.
- The abstraction exposes the embedding width and the model identity, so the
  chunk store can reject a query produced by a different model.
- Tests that require real hardware are marked and excluded from the default run.

### Resource behaviour

- The model is loaded once and reused; no request loads or reloads it.
- Batch size is bounded by a configured limit, and a request larger than the
  limit is split rather than attempted whole, because an out-of-memory failure
  mid-index loses the run.
- Peak GPU memory for the configured maximum batch is measurable and reported.
- Embedding a batch is safe to call concurrently, and concurrent callers do not
  corrupt each other's results.
- Failures — model file absent, GPU unavailable, allocation failure — are
  distinguishable from each other, so the daemon can decide what to do about
  each.

### Latency

- Embedding a single short query completes within the query-embedding portion of
  the project direction's latency budget, and that figure is measured rather
  than asserted.
- Throughput for large batches is measured and reported, because it sets the
  duration of a full repository index.

## Constraints and non-goals

- No reranking, no cross-encoder. Phase 2, gated.
- No search, ranking, or similarity computation. This task turns text into
  vectors; SIFT-008 multiplies them. The dense search matmul is deliberately not
  here, so it can be optimized against the storage layout rather than hidden
  behind an inference call.
- No model downloading, exporting, or conversion. That is SIFT-004, and this
  task consumes its artifacts and its fixture as given.
- No dynamic batching across requests, no request queue, no scheduler. The
  daemon owns concurrency policy in SIFT-010.
- No caching of embeddings by text. Caching is keyed by content hash at the
  storage layer, and duplicating it here would produce two caches that disagree.
- No fallback to CPU inference when the GPU is unavailable. A missing GPU is an
  error the daemon reports; silently running two orders of magnitude slower
  would blow the latency budget while appearing to work.
- No support for multiple models loaded at once. One embedding model at a time.

## Acceptance criteria

### Agent-verifiable

1. Tokenizing every fixture input reproduces the fixture's pinned token
   sequences exactly; this test runs without a GPU.
2. Pooling and normalization are unit tested on synthetic hidden states with
   hand-computed expected values, including a batch containing padding.
3. The mock implementation satisfies the abstraction, is deterministic across
   runs, and returns vectors of the declared width.
4. The workspace builds and the default test suite passes with no GPU runtime,
   no CUDA toolkit, and no exported model present.
5. A request exceeding the configured batch limit is split, and the vectors it
   returns equal those from issuing the sub-batches directly.
6. Absent model files, unavailable GPU, and allocation failure each surface as
   distinguishable errors, asserted by test.
7. Truncation of an over-length input is reported to the caller.
8. The full validation suite passes: `./ci.sh`

### Human-verifiable

1. On the target machine, embedding the fixture inputs through the real GPU path
   reproduces the reference vectors within the fixture's tolerance.  
   Command: `cargo test -p inference --release --features cuda -- --ignored fixture_parity`
2. Single-query embedding latency is measured over at least 100 runs after
   warm-up and the median and 95th percentile are reported against the budget.  
   Command: `cargo run --release -p inference --example bench_embed -- --queries 100`
3. Batch throughput and peak GPU memory at the configured maximum batch size are
   measured with a desktop session attached and reported.  
   Command: `cargo run --release -p inference --example bench_embed -- --batch-sweep --report-vram`
