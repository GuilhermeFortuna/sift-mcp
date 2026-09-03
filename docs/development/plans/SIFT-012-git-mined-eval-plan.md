# SIFT-012 implementation plan: Git-mined evaluation harness

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/SIFT-012-git-mined-eval-spec.md`](../specs/SIFT-012-git-mined-eval-spec.md)  
**Depends on:** SIFT-006, SIFT-009

## Current-system context

`crates/eval` is empty from SIFT-001, which required it to compile without a
GPU — it depends on `inference` only through the `Embedder` trait. The pieces it
needs exist: `indexing::RepoGit` (SIFT-006) opens a repository, resolves
`head_commit`, and returns `FileChange` entries for a commit range;
`indexing::Chunker` (SIFT-003) maps a file at a revision to symbols with line
ranges; `retrieval::Searcher` (SIFT-009) returns `SearchResponse` with
`StageTimings` and per-retriever `Contribution` values that make ablation
possible without a second code path; `storage::ChunkStore::rows_for_file` and
`get_many` resolve rows to records carrying `file` and `symbol`.

The design document's *Evaluation* section specifies the mining rules, the
filters, the metrics, and the proxy KPI this task implements, and warns that a
hand-authored query set is "tiny, biased" and must be a sanity check only.

The corpora available locally were surveyed and measured before this plan was
written. The repositories authored here are small — `job-engine` at 102 commits,
`portfolio-website` at 120, `ai-prompts` at 56, `job-tracker` at 51 — and under
the design document's own filters they yield about 20% of non-merge commits as
labels, roughly 65 labels across all four. At 65 labels a top-3 near 0.80 carries
a 95% confidence interval of about ±0.10, which cannot distinguish 0.75 from
0.85 and therefore cannot support a verdict on the Phase 1 exit criterion.
`~/llama.cpp` is a third-party checkout at 10,688 commits and 3,505 tracked
files; a 600-commit sample survives the same filters at 47%, extrapolating to
roughly 4,000-5,000 labels. The gap this task closes is that retrieval quality
has never been measured, and Phase 1's exit criteria have no instrument with
enough labels to be judged against.

## Interfaces produced

```rust
// crates/eval/src/mine.rs
pub enum LabelSource { CommitSubject, Docstring }

pub struct Label {
    pub query: String,
    /// Expected answers as (file, qualified symbol), matching ChunkRecord.
    pub expected: Vec<(String, String)>,
    pub source: LabelSource,
    pub provenance: String,        // commit hash, or file:symbol for docstrings
}

/// Why a commit was rejected. Counted and reported per rule.
pub enum RejectReason {
    Merge, TooManySymbols { count: usize }, SubjectTooShort { words: usize },
    MaintenancePattern { pattern: &'static str }, NoSymbolsTouched,
    SymbolsNotIndexed { missing: usize },
}

pub struct MiningConfig {
    pub max_symbols_per_commit: usize,   // design document says 1-3
    pub min_subject_words: usize,
    pub maintenance_patterns: Vec<String>,
    pub max_commits: Option<usize>,
}

pub struct MiningReport {
    pub commits_examined: u64,
    pub labels_accepted: u64,
    pub rejected: BTreeMap<String, u64>,  // rule name -> count
}

pub fn mine_commits(repo: &Path, store: &ChunkStore, config: &MiningConfig)
    -> Result<(Vec<Label>, MiningReport), EvalError>;

/// Docstring labels. The held-out set the index must be built without.
pub fn mine_docstrings(repo: &Path, store: &ChunkStore)
    -> Result<(Vec<Label>, MiningReport), EvalError>;
```

```rust
// crates/eval/src/metrics.rs
/// A hit is an expected (file, symbol) appearing in the top n results.
pub struct Metrics {
    pub labels_scored: u64,        // reported beside every figure, per the spec
    pub labels_discarded: u64,     // expected symbols absent from the index
    pub top_1: f64,
    pub top_3: f64,
    pub top_10: f64,
    pub mrr: f64,
    pub latency_p50_ms: f64,
    pub latency_p95_ms: f64,
    pub peak_gpu_bytes: u64,
    pub bytes_before_hit: BytesBeforeHit,
}

pub struct BytesBeforeHit {
    pub mcp_median: u64,
    pub baseline_median: u64,      // grep-style baseline over the same repository
    pub baseline_command: String,
}

/// Nearest-rank on sorted samples, so a reported percentile means one thing.
pub fn percentile(sorted_ms: &[f64], fraction: f64) -> f64;
pub fn reciprocal_rank(ranked: &[(String, String)], expected: &[(String, String)]) -> f64;
```

```rust
// crates/eval/src/run.rs
/// The three configurations the design bets on. No reranking anywhere.
pub enum Ablation { LexicalOnly, DenseOnly, Fused }

/// Everything needed to compare two runs and attribute a difference.
pub struct RunManifest {
    pub repo_commit: String,
    pub indexed_commit: String,
    pub model_id: String,
    pub fusion: retrieval::FusionConfig,
    pub harness_version: u32,
    pub label_set: String,         // "mined" | "docstring" | "handwritten"
    pub timestamp: String,
}

pub struct EvalRun {
    pub manifest: RunManifest,
    pub per_ablation: BTreeMap<Ablation, Metrics>,
    pub by_source: BTreeMap<LabelSource, Metrics>,
    pub by_query_length: BTreeMap<LengthBucket, Metrics>,
}

pub fn evaluate(searcher: &Searcher, labels: &[Label], ablations: &[Ablation])
    -> Result<EvalRun, EvalError>;
```

## Implementation decisions

- **A commit's changed symbols are resolved by intersecting its changed line
  ranges with the chunk line ranges of the file at that revision, not by name
  matching in the diff text.** Name matching in a diff picks up call sites and
  comments; the line-range intersection is what the parse tree already tells us
  and it is the only method that distinguishes "changed this function" from
  "mentioned it".

- **`max_symbols_per_commit` defaults to 3, per the design document's "drop
  commits touching >3 symbols".** A commit touching twenty symbols has a subject
  that describes none of them specifically, so it measures nothing about
  ranking.

- **Every rejection is counted by rule and reported, and the counts are asserted
  to reconcile with the commits examined.** A filter that silently removes 95%
  of history changes what is being measured, and an aggregate rejection count
  cannot show which rule did it.

- **Docstring labels hold their documentation out of the index by building a
  second index with `doc_first_line` and the doc comment excluded from the
  embedded body, rather than by filtering results.** Filtering afterwards does
  not help: the query text is inside the embedding, so the chunk is retrieved
  for the wrong reason and the metric is inflated.

- **A label whose expected symbols are absent from the index is discarded and
  counted in `labels_discarded`, never scored as a miss.** Those measure
  chunking coverage, not ranking, and mixing them makes an indexing regression
  look like a retrieval regression.

- **A hit requires both file and qualified symbol to match.** File-only matching
  credits retrieving any symbol in a large file, which for a thousand-line file
  is nearly free and would inflate top-1 substantially.

- **`percentile` uses nearest-rank on sorted samples, stated here because every
  reported p95 across the project must mean the same thing.** Interpolating
  produces figures that differ from the daemon's own logged percentiles for the
  same data, and reconciling those later is wasted effort.

- **Latency is measured through `Searcher::search` with the real
  `OnnxEmbedder`, and SIFT-013 re-measures the same thing through the socket and
  the spawned client.** This task's figure isolates retrieval; the end-to-end
  SLO belongs to the acceptance task, and conflating them would leave a miss
  unattributable.

- **Ablations are produced by running the same `Searcher` with fusion depths set
  to zero on one side, rather than by separate code paths.** A dedicated
  lexical-only path would not be the code that ships, and the ablation would
  measure something the system never does.

- **No reranking configuration exists in this harness.** The Phase 2 gate needs
  an un-reranked baseline, and a harness that can rerank will be used to rerank
  before the baseline is locked.

- **The proxy KPI counts bytes of code an agent would read before the correct
  symbol enters context: for the MCP path, the serialized bytes of results up to
  and including the first hit; for the baseline, the bytes of the files a
  keyword search returns, in its output order, up to the file containing the
  expected symbol.** The design document defines the measure and defers the
  agent A/B; stating both sides precisely here is what makes the comparison
  reproducible rather than rhetorical.

- **The baseline command is recorded verbatim in the output.** A comparison
  against an unnamed baseline is not a comparison, and the baseline must be
  something the agent genuinely already has.

- **Mining output is a stable, sorted, deterministic artifact and the
  hand-written set is a separate committed file.** The spec requires the
  hand-written set be reported separately and never used as the primary metric;
  keeping it in a different file makes accidentally merging them a visible
  change.

- **`RunManifest` is emitted with every run and `harness_version` is bumped on
  any change to mining or metric definitions.** Two runs that differ because the
  harness changed, rather than because the system did, is the failure this
  prevents.

- **The mined corpus is `llama.cpp` at a pinned commit, not a repository
  authored here.** It is the only corpus available with enough history for the
  accuracy figure to be precise enough to judge against the 0.80 target — the
  local first-party repositories yield roughly 65 labels between them, an
  interval too wide to support a verdict. It is also the fairer test: the design
  document warns that authoring queries for one's own repository produces a
  biased benchmark, and an unfamiliar corpus puts the harness in the agent's
  actual position.

- **The corpus revision is pinned in the harness configuration and recorded in
  `RunManifest`, and the harness refuses to run against a checkout at a
  different revision.** `llama.cpp` is actively developed; a `git pull` between
  two runs would change the label set and the numbers with nothing indicating
  why they moved.

- **No `llama.cpp` content is committed to this repository — not source, not
  commit subjects, not symbol names — and labels are regenerated from the pinned
  revision on demand.** It is a third-party MIT-licensed project, and vendoring
  its content into an unrelated repository is a licensing question this project
  has no reason to take on. The cost is that mining reruns before each
  evaluation, which is seconds.

- **The docstring and hand-written sets are drawn from the first-party
  repositories instead, and results are reported per corpus rather than
  merged.** `llama.cpp` is C, C++, CUDA and Metal; the code this project will
  actually be pointed at is TypeScript, JavaScript and Python. The docstring
  source scales with symbol count rather than commit count, so a 567-file or
  4,344-file repository supplies it adequately despite a short history. Merging
  the two into one aggregate would report a retrieval quality measured on one
  language family as though it transferred to another.

- **The corpus and its revision are fixed before the first measured run and are
  not changed afterward.** Selecting the corpus that scores best is fitting the
  benchmark to the system, which destroys the meaning of the Phase 2 gate that
  depends on it.

- **No tuning happens in this task.** The spec makes it a non-goal; a
  configuration change made while looking at the numbers cannot then be
  evaluated by them.

## Ordered implementation

1. Create the branch `SIFT-012-git-mined-eval`.
2. Add dependencies on `storage`, `indexing`, `retrieval`, and `inference` with
   default features to `crates/eval`, plus `serde_json` and `regex`. Confirm
   `cargo build --workspace` succeeds with no GPU present. Commit.
2a. Record the corpus configuration: the mined corpus path and its pinned
   revision, and the first-party repositories used for the docstring and
   hand-written sets. Write a failing test asserting the harness refuses to mine
   a checkout whose `HEAD` differs from the pinned revision, naming both. Run
   and confirm it fails. Implement the check. Confirm it passes. Commit.
3. Write failing unit tests for `percentile`: p95 of 1..=100 is 95; a
   single-sample slice returns that sample; an empty slice returns 0.0; p50 of
   an even-length slice uses nearest-rank rather than interpolating. Run and
   confirm they fail. Implement. Confirm they pass. Commit.
4. Write failing unit tests for `reciprocal_rank` and top-k accuracy on
   hand-built rankings: expected at rank 1 gives 1.0; at rank 4 gives 0.25;
   absent gives 0.0; an empty ranking gives 0.0; with two expected symbols, the
   first to appear determines the rank; a hit requires both file and symbol to
   match, asserted by a case where only the file matches. Run and confirm they
   fail. Implement. Confirm they pass. Commit.
5. Build a fixture repository with a known history covering each filter case: a
   merge, a commit touching five symbols, a two-word subject, subjects matching
   `wip`, `fixup`, `typo`, `bump`, and `lint`, a commit touching no symbols, and
   three commits that should be accepted. Write a failing test asserting
   `mine_commits` accepts exactly those three and that each rejection is
   attributed to the right rule. Run and confirm it fails. Implement mining with
   line-range intersection. Confirm it passes. Commit.
6. Write a failing test asserting `MiningReport` reconciles: accepted plus the
   sum of rejections equals commits examined. Run and confirm it fails.
   Implement the accounting. Confirm it passes. Commit.
7. Write a failing determinism test: mining the same history twice yields
   byte-identical label output. Run and confirm it fails. Sort the output by
   provenance. Confirm it passes. Commit.
8. Write a failing test for docstring labels: the held-out index does not
   contain the docstring text, asserted by searching the held-out index for a
   phrase unique to a docstring and getting no lexical hit for that reason; the
   docstring label's expected symbol is the documented one. Run and confirm it
   fails. Implement `mine_docstrings` and the held-out index build. Confirm it
   passes. Commit.
9. Write a failing test asserting labels whose expected symbols are absent from
   the index are discarded into `labels_discarded` and excluded from every
   accuracy figure. Run and confirm it fails. Implement the discard path.
   Confirm it passes. Commit.
10. Write a failing test asserting every reported metric carries
    `labels_scored`, and that a metric computed over zero labels reports zero
    scored rather than a NaN or a silent 0.0 accuracy. Run and confirm it fails.
    Implement. Confirm it passes. Commit.
11. Write a failing integration test with `MockEmbedder` over the fixture
    repository: `evaluate` with all three ablations produces three `Metrics`
    entries, the lexical-only and dense-only runs differ from fused, and each
    reports its own latency. Run and confirm it fails. Implement `evaluate` via
    fusion-depth configuration rather than separate paths. Confirm it passes.
    Commit.
12. Write a failing test for breakdowns: with labels from both sources and of
    varying length, `by_source` and `by_query_length` partition the labels
    exactly once each and their scored counts sum to the overall count. Run and
    confirm it fails. Implement the breakdowns. Confirm it passes. Commit.
13. Write a failing test for the proxy KPI on a constructed case with
    hand-computed byte counts for both the MCP path and the baseline, asserting
    the baseline command string is recorded verbatim. Run and confirm it fails.
    Implement `BytesBeforeHit`. Confirm it passes. Commit.
14. Write a failing test asserting `RunManifest` is emitted with every field
    populated and that `harness_version` matches a constant, and that the
    machine-readable output round-trips. Run and confirm it fails. Implement
    serialization. Confirm it passes. Commit.
15. Write the hand-written sanity set as a separate committed file of about 30
    natural questions with expected symbols for a chosen repository, and a test
    asserting it is loaded only under `--set handwritten` and never merged into
    the mined set. Commit.
16. Add the `mine`, `evaluate`, and `proxy_kpi` examples with the flags the spec
    names: `--report`, `--ablations`, and `--set`. Commit.
17. Human step: run `cargo run --release -p eval --example mine --
    ~/llama.cpp --report` at the pinned revision, and report total yield, the
    achieved label count with the confidence interval it implies for a top-3
    near 0.80, per-rule rejection counts, and a read sample of accepted pairs
    judged for whether they are genuinely answerable from the code.
18. Human step: run `cargo run --release -p eval --example evaluate --
    <store-path> --ablations` and report top-1, top-3, top-10, MRR, p50 and p95
    latency, peak GPU memory, and label counts for each of the three ablations.
19. Human step: run `cargo run --release -p eval --example proxy_kpi --
    <repo-path> <store-path>` and report median bytes before hit for the MCP
    path and for the baseline, with the baseline command.
20. Human step: run `cargo run --release -p eval --example evaluate --
    <store-path> --set handwritten` and report its results separately, as a
    sanity check only.
21. Human step: build a store over a first-party repository in this project's
    working languages and run `cargo run --release -p eval --example evaluate --
    <store-path> --set docstring`, reporting its figures beside the mined
    corpus's rather than merged, and noting any gap between the two.
22. Run the full validation suite and confirm it passes.

## Validation

- **Unit:** `percentile` nearest-rank behaviour; `reciprocal_rank` and top-k
  including ties, absence, and file-only near-misses; mining report
  reconciliation.
- **Integration:** mining over a fixture history exercising every filter rule;
  determinism across two runs; docstring hold-out; discard accounting;
  three-ablation evaluation with `MockEmbedder`; breakdown partitioning; proxy
  KPI on a hand-computed case; manifest round trip.
- **Regression:** the fixture history's expected label set is the locked
  reference for mining; a change to filters must show as a diff there and bump
  `harness_version`.
- **Manual:** reading a sample of mined pairs for answerability, and reading the
  hand-written set's results; correct means the mined pairs are questions the
  code genuinely answers, and the hand-written results are not used to justify
  any change.
- **Measurement:** top-1, top-3, top-10, MRR, p50 and p95 latency, peak GPU
  memory, and scored-label counts for each of lexical-only, dense-only, and
  fused, over the full mined set on the target machine; median bytes before hit
  for the MCP path against the keyword baseline.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
./ci.sh
cargo run --release -p eval --example mine -- ~/llama.cpp --report
cargo run --release -p eval --example evaluate -- <store-path> --ablations
cargo run --release -p eval --example proxy_kpi -- <repo-path> <store-path>
cargo run --release -p eval --example evaluate -- <store-path> --set handwritten
```

## Handoff

Report the mined corpus and its pinned revision; commits examined, labels
accepted, and the rejection count for every rule with the reconciliation
check; the achieved label count and the confidence interval it implies for a
top-3 near the 0.80 target, stated explicitly as sufficient or insufficient to
support a verdict; the number of labels discarded for absent symbols
and what that implies about chunking coverage; top-1, top-3, top-10, MRR, p50
and p95 latency, peak GPU memory, and scored-label count for each of the three
ablations, stated separately so the contribution of each retrieval path is
visible; the same figures broken down by label source and by query length,
noting whether one source carries the aggregate; median bytes before hit for the
MCP path and for the baseline with the baseline command verbatim; the
hand-written set's results reported separately and explicitly labelled as a
sanity check; the docstring set's figures over a first-party repository
reported beside the mined corpus's, with any language gap noted; the `harness_version` and the full `RunManifest` of the reported
run; and an explicit statement that no configuration was tuned in response to
these numbers.
