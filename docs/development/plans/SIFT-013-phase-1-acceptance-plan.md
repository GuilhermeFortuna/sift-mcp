# SIFT-013 implementation plan: Phase 1 acceptance and locked baseline

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/SIFT-013-phase-1-acceptance-spec.md`](../specs/SIFT-013-phase-1-acceptance-spec.md)  
**Depends on:** SIFT-011, SIFT-012

## Current-system context

Every Phase 1 component is built and each has been measured in isolation.
`eval::evaluate` (SIFT-012) produces an `EvalRun` with per-ablation `Metrics`
and a `RunManifest` carrying repository commit, indexed commit, model id, fusion
configuration, and `harness_version`, and its latency figures come from
`Searcher::search` in process. `SiftMcpServer` (SIFT-011) serves four tools over
stdio and `scripts/time-cold-start.sh` measures spawn to handshake.
`daemon::DaemonClient` (SIFT-010) connects or spawns, and every daemon request
already logs `StageTimings`. `DaemonStatus::resident_gpu_bytes` reports GPU
residency.

What does not exist: any measurement taken through the agent-facing path — the
spawned MCP client, the socket, the daemon — and any artifact that pins a figure
so a later run can be compared against it mechanically. The design document's
Phase 1 exit criteria are top-3 at or above 0.80, `search_code` p95 under
400 ms end to end, and cold agent start under 200 ms, and Phase 2's reranker is
gated on beating this task's top-1. The gap this task closes is that Phase 1 has
never been judged, and Phase 2 has nothing to be gated against.

## Interfaces produced

```rust
// crates/eval/src/baseline.rs
/// Locked once, in baselines/SIFT-013/, never edited afterward.
pub struct LockedBaseline {
    pub commit: String,              // full sha the measurement was taken on
    pub manifest: eval::RunManifest,
    pub per_ablation: BTreeMap<Ablation, Metrics>,
    pub exit_criteria: ExitCriteria,
    pub cold_start_ms: LatencySamples,
    pub end_to_end_ms: LatencySamples,
    pub peak_gpu_bytes: u64,
}

/// Each criterion carries measured, target, and an explicit verdict. A missed
/// criterion is recorded as missed, never rounded toward the target.
pub struct ExitCriteria {
    pub top_3: Criterion,            // target 0.80
    pub search_p95_ms: Criterion,    // target 400.0
    pub cold_start_p95_ms: Criterion,// target 200.0
}

pub struct Criterion { pub measured: f64, pub target: f64, pub met: bool }

pub struct LatencySamples {
    pub runs: usize,
    pub warmup: usize,
    pub p50: f64,
    pub p95: f64,
}

pub fn write_locked(path: &Path, baseline: &LockedBaseline) -> Result<(), EvalError>;
pub fn read_locked(path: &Path) -> Result<LockedBaseline, EvalError>;

/// Per-metric deltas, for the Phase 2 gate and for regression checks.
pub struct BaselineDelta { pub metric: String, pub baseline: f64, pub current: f64, pub delta: f64 }
pub fn compare(baseline: &LockedBaseline, current: &EvalRun) -> Vec<BaselineDelta>;
```

```rust
// crates/eval/src/agent_path.rs
/// Drives the real MCP client over stdio, so measurements include the transport
/// the SLO is stated for. Not a shortcut into Searcher.
pub struct AgentPathHarness { /* spawned mcp-client process, stdio pipes */ }

impl AgentPathHarness {
    pub fn spawn(store_dir: &Path) -> Result<Self, EvalError>;
    pub fn search(&mut self, query: &str, top_k: usize) -> Result<(Vec<SearchResult>, f64), EvalError>;
}
```

## Implementation decisions

- **Accuracy and latency are re-measured through `AgentPathHarness`, not reused
  from SIFT-012's in-process figures.** The SLO is stated end to end and the
  spec requires the full path; a p95 that excludes serialization, the socket,
  and process boundaries is not the number the criterion names.

- **Latency is reported as p50 and p95 over at least 200 queries after a stated
  warm-up, using `eval::percentile`'s nearest-rank definition.** Reusing the
  existing function is what makes this figure comparable to SIFT-012's and to
  the daemon's own logs; a second percentile definition would produce three
  numbers for one quantity.

- **Cold start is measured twice — daemon warm and daemon cold — and the
  criterion is judged against the warm figure.** The design document's 200 ms
  budget is for agent start, and the daemon exists precisely so the model load
  is not on that path. Reporting only the warm figure would hide the cost a user
  actually pays on the first session of the day; judging against the cold figure
  would judge the criterion against something it was not written for. Both are
  recorded and the distinction is stated.

- **`Criterion` stores measured, target, and `met` computed by comparison rather
  than asserted.** A hand-written verdict drifts from its own number under
  editing, and the spec forbids rounding toward the target.

- **The baseline is written once and the file is treated as locked: later tasks
  read it and never rewrite it.** A baseline that can be regenerated is not a
  baseline — the Phase 2 gate depends on comparing against a figure that
  predates the change being judged.

- **The baseline records the full commit sha and the complete `RunManifest`.**
  Numbers without a commit are anecdotes, and without the manifest a
  disagreeing rerun cannot be attributed to code, index, model, or configuration.

- **Per-ablation figures are locked alongside the fused ones.** The Phase 2 gate
  needs the fused top-1 to beat, and the ablations are the evidence for whether
  both retrieval paths earn their place — a question that will be asked again
  when reranking competes for the same VRAM.

- **The record states the margin Phase 2 must beat as a number derived from the
  locked top-1, rather than leaving "material" undefined.** The design document
  says reranking is kept only if top-1 improves materially at acceptable
  latency; an undefined threshold is decided after the fact by whoever built the
  reranker.

- **The acceptance record is written with its manual section laid out and empty
  before any human run happens.** The spec's human-verifiable criteria map
  one-to-one onto its entries, so the human knows what will be asked and the
  record cannot acquire a criterion after the fact to fit a result.

- **No optimization, no tuning, no configuration change is made in this task,
  even if a criterion is missed.** The spec makes this the strongest non-goal: a
  fix applied in the measuring change makes the record describe a system that
  was never measured. A miss produces a finding and a follow-up row with its
  triggering measurement.

- **Accuracy is measured over SIFT-012's pinned mined corpus, and the locked
  baseline records that corpus and its revision.** A baseline whose corpus is
  not pinned cannot be compared against, which would leave the Phase 2 gate
  measuring the corpus rather than the reranker.

- **The reported top-3 carries its confidence interval, and a verdict is
  withheld when the interval spans the target.** The design document's exit
  criterion is a threshold; declaring it met on a point estimate whose interval
  includes 0.79 would lock in a baseline that Phase 2 is then measured against
  as though it were precise.

- **Deferred follow-ups each carry the measurement that would justify them.**
  A follow-up with no triggering measurement is speculation, and this record is
  what Phase 2 and Phase 3 will be planned from.

- **`STATUS.md` is moved to `DONE` for the batch only after every pending manual
  result is filled in.** The record exists to make an incomplete acceptance
  visible rather than letting "the code is finished" stand in for "the milestone
  is accepted".

## Ordered implementation

1. Create the branch `SIFT-013-phase-1-acceptance`.
2. Write failing unit tests for `Criterion` and `ExitCriteria`: a measured 0.81
   against a target 0.80 is met; 0.799 is not met and is recorded as measured
   rather than rounded; a latency criterion is met when measured is *below*
   target, asserting the comparison direction differs from the accuracy one. Run
   and confirm they fail. Implement. Confirm they pass. Commit.
3. Write failing tests for `write_locked` and `read_locked`: a constructed
   baseline round-trips with every field intact including the commit sha and the
   full manifest; reading a file whose `harness_version` differs from the current
   one returns an error naming both. Run and confirm they fail. Implement.
   Confirm they pass. Commit.
4. Write failing tests for `compare` on a constructed baseline and a constructed
   current run: deltas are computed per metric with the correct sign; a metric
   present in one and absent in the other is reported rather than skipped; an
   identical pair yields all-zero deltas. Run and confirm they fail. Implement.
   Confirm they pass. Commit.
5. Write a failing integration test for `AgentPathHarness` using a daemon backed
   by `MockEmbedder`: spawning the real MCP client, issuing a search over stdio,
   and receiving results equal to `Searcher::search` in process for the same
   store, with a per-call latency recorded. Run and confirm it fails. Implement
   the harness. Confirm it passes. Commit.
6. Write a failing test asserting the harness measures the full path: with the
   daemon deliberately delayed, the harness's recorded latency reflects the
   delay, proving it is not shortcutting into the library. Run and confirm it
   fails. Confirm the harness drives the spawned process. Confirm it passes.
   Commit.
7. Add `scripts/acceptance/measure-accuracy.sh`, `measure-latency.sh`,
   `measure-cold-start.sh`, `measure-vram.sh`, and `measure-ablations.sh`, each
   emitting machine-readable output that feeds `write_locked`. Have each script
   record the commit sha it ran on and refuse to run on a dirty worktree, since
   a figure from an uncommitted tree cannot be reproduced. Commit.
8. Copy `templates/acceptance.md` to `baselines/SIFT-013/acceptance.md` and fill
   in everything that does not require a human run: the environment table, the
   automated validation groups with their reproducing commands, and the manual
   section laid out with one entry per human-verifiable criterion from the
   SIFT-013 spec, each carrying its command, the figure it is compared against,
   and `Result: _pending_`. Commit.
9. Add to the record a section enumerating every human-verifiable criterion from
   SIFT-001 through SIFT-012, each paired with the handoff report that recorded
   its evidence or marked explicitly as outstanding. Write a test asserting the
   enumeration covers every human-verifiable criterion in every spec file, so a
   criterion cannot be quietly omitted. Run it against a deliberately removed
   entry and confirm it fails, restore it, and confirm it passes. Commit.
10. Human step: run `scripts/acceptance/measure-accuracy.sh <mined-corpus>
    <store-path>` against SIFT-012's pinned corpus and record top-3 with its
    scored-label count, its confidence interval, and the corpus revision, against
    the 0.80 target.
11. Human step: run `scripts/acceptance/measure-latency.sh <store-path>
    --queries 200` and record p50, p95, and the per-stage split against the
    400 ms target.
12. Human step: run `scripts/acceptance/measure-cold-start.sh` over at least 20
    runs with the daemon warm and again with it cold, and record both against
    the 200 ms target.
13. Human step: run `scripts/acceptance/measure-vram.sh <repo-path>
    <store-path>` under a concurrent search-and-index workload with a desktop
    session attached, and record peak GPU memory against the ~5.0 GB budget.
14. Human step: run `scripts/acceptance/measure-ablations.sh <store-path>` and
    record lexical-only, dense-only, and fused figures, judging whether both
    retrieval paths earn their place.
15. Fill every pending result in the acceptance record from steps 10–14, write
    the locked baseline into `baselines/SIFT-013/` from the same runs, and state
    the explicit met-or-not-met verdict for each of the three exit criteria.
    Commit.
16. Write the deferred follow-ups table, one row per finding from the
    measurements, each with the measurement that would justify acting on it, and
    state the top-1 margin Phase 2 must beat. Commit.
17. Update `STATUS.md` to reflect the measured outcome for the batch, moving
    tasks to `DONE` only where their human-verifiable criteria have recorded
    evidence. Commit.
18. Run the full validation suite and confirm it passes.

## Validation

- **Unit:** `Criterion` comparison direction for accuracy and latency targets;
  baseline round trip and version mismatch; `compare` deltas including
  asymmetric metric sets.
- **Integration:** `AgentPathHarness` results equal to in-process results for
  the same store; the delayed-daemon test proving the harness measures the full
  path; the coverage test asserting the record enumerates every
  human-verifiable criterion across all thirteen specs.
- **Regression:** none to compare against — this task *creates* the locked
  reference. From here on, `baselines/SIFT-013/` is the file every later
  retrieval change is diffed against with `compare`.
- **Manual:** the five measurement scripts, run on the target machine with a
  desktop session attached; correct means each produces a figure with its run
  count and a recorded commit sha, and refuses to run on a dirty worktree.
- **Measurement:** top-3 accuracy with scored-label count against 0.80;
  end-to-end p50 and p95 over at least 200 queries after warm-up, with the
  per-stage split, against 400 ms; cold start p50 and p95 over at least 20 runs,
  warm and cold, against 200 ms; peak GPU memory under concurrent
  search-and-index against ~5.0 GB; all three ablations through the same path.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
./ci.sh
scripts/acceptance/measure-accuracy.sh <mined-corpus> <store-path>
scripts/acceptance/measure-latency.sh <store-path> --queries 200
scripts/acceptance/measure-cold-start.sh
scripts/acceptance/measure-vram.sh <repo-path> <store-path>
scripts/acceptance/measure-ablations.sh <store-path>
```

## Handoff

Report the commit sha every measurement was taken on and the full `RunManifest`;
top-3 accuracy with its scored-label count, its confidence interval, the
corpus name and revision it was measured over, and an explicit met-or-not-met
verdict against 0.80 — or an explicit statement that the interval spans the
target and supports no verdict; end-to-end p50 and p95 over at least 200 queries with the
per-stage split and a verdict against 400 ms; cold start p50 and p95 over at
least 20 runs, reported separately for a warm and a cold daemon, with a verdict
against 200 ms judged on the warm figure; peak GPU memory under concurrent
search and index against the ~5.0 GB budget; the three ablation results and a
judgement on whether both retrieval paths earn their place; the top-1 figure now
locked and the margin Phase 2 reranking must beat; confirmation that the
acceptance record enumerates every human-verifiable criterion from SIFT-001
through SIFT-012 with its evidence or an explicit outstanding note; the deferred
follow-ups with the measurement justifying each; and — if any criterion was
missed — the measured value, the stage the evidence points to, and confirmation
that nothing was tuned in this task to close the gap.
