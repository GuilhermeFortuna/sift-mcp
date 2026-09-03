# SIFT-005 implementation plan: Embedding inference

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/SIFT-005-embedding-inference-spec.md`](../specs/SIFT-005-embedding-inference-spec.md)  
**Depends on:** SIFT-004

## Current-system context

`crates/inference` exists from SIFT-001 as an empty library with a default-off
`cuda` feature that gates an optional `ort` dependency, and one `#[ignore]`d
placeholder test. SIFT-004 produced, per model key, an ONNX graph returning
per-token hidden states, the tokenizer files, a `metadata.json` carrying
`model_id`, `dims`, `max_sequence_length`, `pooling`, `normalize`, and the
prefix conventions, and — committed at
`crates/inference/fixtures/<key>-reference.json` — token sequences and reference
vectors with a measured tolerance. Nothing loads any of it.

`storage::MatrixHeader` from SIFT-002 has `dims` and `model_id` fields that must
be filled from this crate's metadata, and it rejects a vector of the wrong width
and a query from a mismatched model. The gap this task closes is that no text
can be turned into a vector, and every downstream crate would otherwise have to
depend on the GPU runtime to be tested at all.

## Interfaces produced

```rust
// crates/inference/src/lib.rs
/// The abstraction every consumer depends on. `retrieval`, `indexing`, and
/// `eval` name this trait and never name `ort`.
pub trait Embedder: Send + Sync {
    fn model_id(&self) -> &str;
    fn dims(&self) -> u32;
    /// Splits internally at the configured batch limit. Output order matches
    /// input order. `role` selects the prefix convention from metadata.
    fn embed(&self, texts: &[&str], role: Role) -> Result<Vec<Embedding>, InferError>;
}

/// Queries and documents differ only by prefix, applied from metadata.
pub enum Role { Query, Document }

pub struct Embedding {
    pub vector: Vec<f16>,   // pooled, L2-normalized, length == dims
    pub truncated: bool,    // input exceeded max_sequence_length
}

pub enum InferError {
    ModelFilesMissing { path: PathBuf },
    GpuUnavailable { detail: String },
    Allocation { requested_bytes: u64 },
    ArtifactHashMismatch { expected: String, got: String },
    Tokenizer(tokenizers::Error),
    Runtime(String),
}
```

```rust
// crates/inference/src/metadata.rs
/// Deserialized from models/<key>/metadata.json, written by SIFT-004.
pub struct ModelMetadata {
    pub model_id: String,
    pub dims: u32,
    pub max_sequence_length: usize,
    pub pooling: Pooling,
    pub normalize: Normalize,
    pub query_prefix: Option<String>,
    pub document_prefix: Option<String>,
    pub onnx_sha256: String,
    pub tokenizer_sha256: String,
}

pub enum Pooling { LastToken, Mean, Cls }
pub enum Normalize { L2, None }
```

```rust
// crates/inference/src/pooling.rs
/// hidden: [batch, seq, dims] row-major. mask: [batch, seq], 1 for real tokens.
/// Returns [batch, dims]. Padded positions never contribute.
pub fn pool(hidden: &[f32], mask: &[u32], batch: usize, seq: usize, dims: usize,
            strategy: Pooling) -> Vec<f32>;

/// In-place L2 normalization per row. A zero row is left as zeros rather than
/// producing NaN.
pub fn l2_normalize_rows(vectors: &mut [f32], dims: usize);
```

```rust
// crates/inference/src/onnx.rs   (behind feature = "cuda")
pub struct OnnxEmbedder { /* ort Session, Tokenizer, ModelMetadata, batch limit */ }

impl OnnxEmbedder {
    pub fn load(model_dir: &Path, max_batch: usize) -> Result<Self, InferError>;
    pub fn peak_gpu_bytes(&self) -> u64;
}
impl Embedder for OnnxEmbedder { /* elided */ }
```

```rust
// crates/inference/src/mock.rs
/// Deterministic, CPU-only, no model files. Same text always yields the same
/// vector; different texts yield different vectors.
pub struct MockEmbedder { /* seed, dims, model_id */ }

impl MockEmbedder {
    pub fn new(dims: u32) -> Self;
    /// Returns a vector that is nearest to `text`'s own, for tests that need to
    /// construct a query with a known correct answer.
    pub fn query_matching(&self, text: &str) -> Vec<f16>;
}
impl Embedder for MockEmbedder { /* elided */ }
```

## Implementation decisions

- **`Embedder` is a trait in the crate root, and `OnnxEmbedder` lives behind the
  `cuda` feature while `MockEmbedder` does not.** Consumers depend on
  `crates/inference` with default features and get the trait and the mock; the
  workspace therefore builds and tests with no ONNX Runtime present, which is
  the CPU-only CI requirement from SIFT-001.

- **Pooling and normalization are separate free functions taking plain slices,
  not methods on the session.** They are the numerically dangerous part and they
  need to be unit tested against hand-computed values on synthetic input, which
  is impossible if they can only run behind a loaded model.

- **Pooling accumulates in f32 and only the final normalized vector is narrowed
  to f16.** Summing hidden states at half precision over a long sequence loses
  low-order bits at a rate that shifts the pooled direction; the storage format
  is fp16 but the arithmetic reaching it need not be.

- **`pool` takes the attention mask and padded positions contribute nothing —
  for `LastToken`, the last real token is selected per row, not the last
  column.** Selecting the last column returns the embedding of a pad token for
  every sequence shorter than the batch maximum, which is most of them. This is
  precisely the failure the spec's padding-invariance requirement targets.

- **`l2_normalize_rows` leaves an all-zero row as zeros instead of dividing by
  zero.** The empty-string fixture case can pool to near-zero; a NaN vector
  written into the matrix poisons every subsequent similarity computation with
  no error at the point of failure.

- **`Role` selects the prefix from metadata rather than the caller passing a
  prefix string.** If the prefix is a call-site argument, the index and the
  query paths will eventually disagree, and the symptom is uniformly mediocre
  retrieval rather than a failure.

- **`embed` splits at the configured batch limit internally and the split is
  invisible to the caller.** A caller that must batch correctly is a caller that
  will eventually not, and the failure mode is an out-of-memory abort partway
  through a long index run.

- **Truncation is reported per input on `Embedding` rather than logged.** The
  indexer needs it to decide whether the chunker's oversize threshold from
  SIFT-003 is set correctly, and a log line is not a signal a test can assert.

- **The model artifacts' hashes from SIFT-004's metadata are checked at load.**
  A graph replaced by a re-export that was never verified produces vectors that
  are wrong within tolerance-of-nothing; the hash is the only cheap check.

- **`GpuUnavailable`, `ModelFilesMissing`, and `Allocation` are distinct
  variants.** The daemon in SIFT-010 must respond differently to each — refuse
  to start, refuse to start with a different message, or reduce batch size — and
  a single opaque error forces it to guess.

- **No CPU execution-provider fallback is registered.** ONNX Runtime will
  silently fall back to CPU if the CUDA provider fails to initialize, which
  turns a hard failure into a system that appears to work at roughly a hundred
  times the latency budget. The provider list is CUDA-only and a failure to
  initialize is `GpuUnavailable`.

- **`MockEmbedder` derives a vector by hashing the text into a seeded generator
  and normalizing.** It needs only determinism and distinctness, and
  `query_matching` returns the same vector for a text so that SIFT-008 can build
  a query with a known nearest neighbour without a model.

- **GPU tests are `#[ignore]`d and named for the thing they check, so
  `--ignored fixture_parity` selects the parity test alone.** The spec's
  human-verifiable criterion names that command, and a test named
  `gpu_tests` would run the benchmarks too.

## Ordered implementation

1. Create the branch `SIFT-005-embedding-inference`.
2. Declare `tokenizers`, `half`, `serde`, `serde_json`, and `thiserror` in
   `crates/inference` with default features, and `ort` as optional under
   `cuda`. Confirm `cargo build --workspace` succeeds with no ONNX Runtime
   present. Commit.
3. Write failing tests for `ModelMetadata` deserialization against a committed
   sample metadata file: every field parses, `pooling` maps to `LastToken`, an
   unknown pooling string is an error, and a missing required field is an error.
   Run and confirm they fail. Implement the types. Confirm they pass. Commit.
4. Write failing unit tests for `pool` with `Mean` on synthetic hidden states:
   batch 2, seq 3, dims 2, with the second row masked to length 1, asserting
   hand-computed expected values, and asserting the masked row's result equals
   the same row embedded alone at seq 1. Run and confirm they fail. Implement
   `Mean`. Confirm they pass. Commit.
5. Write failing tests for `LastToken` on the same synthetic input, asserting
   the selected vector is the last *unmasked* position per row, and a test that
   would fail if the last column were selected instead. Run and confirm they
   fail. Implement `LastToken` and `Cls`. Confirm they pass. Commit.
6. Write failing tests for `l2_normalize_rows`: a row of `[3, 4]` normalizes to
   `[0.6, 0.8]`; an all-zero row stays all-zero and contains no NaN; rows are
   normalized independently. Run and confirm they fail. Implement. Confirm they
   pass. Commit.
7. Write failing tests for `MockEmbedder`: it satisfies `Embedder`; the same
   text yields identical vectors across two constructions; two different texts
   yield different vectors; every vector has length `dims` and unit norm within
   tolerance; `query_matching(t)` is nearer to `embed(t)` than to any other
   fixture text's embedding. Run and confirm they fail. Implement. Confirm they
   pass. Commit.
8. Write a failing tokenizer-parity test — CPU-only, no GPU — asserting that
   tokenizing each fixture case reproduces the fixture's pinned token sequence
   exactly, and that the over-length case truncates to `max_sequence_length`.
   Run and confirm it fails. Implement tokenization with the prefix convention
   applied by `Role`. Confirm it passes. Commit.
9. Write failing tests for batch splitting using `MockEmbedder` with a limit of
   4: embedding 10 texts returns 10 vectors in input order, equal to the
   concatenation of embedding them in sub-batches of 4, 4, and 2. Run and
   confirm they fail. Implement splitting in the shared `embed` wrapper.
   Confirm they pass. Commit.
10. Write failing tests for error variants: loading from a directory with no
    graph returns `ModelFilesMissing` naming the path; loading with a metadata
    hash that does not match the file returns `ArtifactHashMismatch` naming
    both. Run and confirm they fail. Implement load-time checks. Confirm they
    pass. Commit.
11. Implement `OnnxEmbedder` behind `cuda`: session creation with a CUDA-only
    provider list, input binding with padding and mask, hidden-state extraction,
    then the already-tested `pool` and `l2_normalize_rows`. Add an `#[ignore]`d
    test `gpu_unavailable_is_distinguishable` asserting a forced provider
    failure yields `GpuUnavailable`. Confirm `cargo build --workspace` still
    succeeds without the feature. Commit.
12. Write the `#[ignore]`d test `fixture_parity`: embed every fixture case
    through `OnnxEmbedder` and assert cosine distance to the reference vector is
    within the fixture's stated tolerance, reporting the per-case distance on
    failure. Commit.
13. Add the `bench_embed` example: `--queries N` measures single-query latency
    over N runs after a warm-up, reporting median and 95th percentile;
    `--batch-sweep` measures throughput across batch sizes; `--report-vram`
    reports peak GPU memory at the configured maximum batch. Commit.
14. Human step: run
    `cargo test -p inference --release --features cuda -- --ignored fixture_parity`
    on the target machine and report the per-case cosine distances against the
    fixture tolerance.
15. Human step: run `cargo run --release -p inference --example bench_embed --
    --queries 100` and report median and 95th percentile single-query latency
    against the design document's query-embedding budget.
16. Human step: run `cargo run --release -p inference --example bench_embed --
    --batch-sweep --report-vram` with a desktop session attached and report
    throughput per batch size and peak GPU memory.
17. Run the full validation suite and confirm it passes.

## Validation

- **Unit:** `pool` for all three strategies on synthetic hidden states with
  hand-computed values, including a padded batch; `l2_normalize_rows` including
  the zero row; metadata deserialization; mock determinism and distinctness.
- **Integration:** tokenizer parity against every fixture case, CPU-only; batch
  splitting equivalence through the mock; load-time error variants.
- **Regression:** the SIFT-004 fixture is the locked reference; `fixture_parity`
  is the check that this crate has not drifted from it.
- **Manual:** `fixture_parity` on the target GPU; correct means every case
  within the fixture's stated tolerance, with per-case distances reported.
- **Measurement:** single-query latency over at least 100 runs after warm-up,
  median and 95th percentile, against the query-embedding budget; batch
  throughput across a sweep; peak GPU memory at the configured maximum batch
  with a desktop session attached.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
./ci.sh
cargo test -p inference --release --features cuda -- --ignored fixture_parity
cargo run --release -p inference --example bench_embed -- --queries 100
cargo run --release -p inference --example bench_embed -- --batch-sweep --report-vram
```

## Handoff

Report per-case cosine distance from the reference fixture and the tolerance
they were judged against; confirmation that tokenization reproduced every pinned
token sequence exactly and that the over-length case truncated as recorded;
evidence that padding does not affect a vector, citing the padded-batch pooling
test; median and 95th percentile single-query latency over at least 100 runs
against the query-embedding budget; batch throughput across the sweep and the
batch limit chosen, with the peak GPU memory at that limit measured with a
desktop session attached and compared against the ~5.0 GB budget; confirmation
that `cargo build --workspace` and `cargo test --workspace` succeed with no ONNX
Runtime, no CUDA toolkit, and no exported model present; and confirmation that
no CPU execution provider is registered, citing the `GpuUnavailable` test.
