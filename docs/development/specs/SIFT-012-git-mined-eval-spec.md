# SIFT-012: Git-mined evaluation harness

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Project direction:** [`../../cuda-mcp-rtx2060-plan.md`](../../cuda-mcp-rtx2060-plan.md), [`../../tech-stack.md`](../../tech-stack.md)  
**Depends on:** SIFT-006, SIFT-009  
**Implementation plan:** [`../plans/SIFT-012-git-mined-eval-plan.md`](../plans/SIFT-012-git-mined-eval-plan.md)

## Purpose

Retrieval quality is currently a matter of opinion, and the obvious remedy —
writing thirty queries by hand — produces a benchmark biased toward the phrasing
of whoever wrote it and too small to distinguish a real improvement from noise.
Git history already contains thousands of labelled pairs: a commit subject is a
natural-language description, and the symbols that commit changed are the
correct answer. This task mines those labels, measures the retrieval system
against them, and produces the numbers that decide whether Phase 1 has met its
exit criteria and whether Phase 2 is worth building.

## Requirements

### Label mining

- Labels are derived from commits, pairing a natural-language string with the
  set of symbols the commit changed, resolved to chunks in the index.
- Commits are filtered before use, at minimum excluding merges, commits touching
  more than a small number of symbols, subjects shorter than a few words, and
  subjects matching maintenance patterns such as work-in-progress, fix-up, typo,
  dependency bump, and lint.
- The filtering rules are stated in one place with the reason for each, and the
  count of commits rejected by each rule is reported, because a filter that
  silently removes most of the corpus changes what is being measured.
- Documented symbols supply a second label source, pairing a symbol's
  documentation with that symbol, and that documentation is held out of the
  index for the queries that use it — an index containing the query text makes
  the measurement meaningless.
- A label whose expected symbols are not present in the index is discarded and
  counted, not scored as a miss, since it measures indexing coverage rather than
  retrieval.
- Mining is deterministic and regenerates identically from the same history at
  the same revision.

### Corpora

- The mined corpus is named explicitly and pinned to a fixed revision, because
  a corpus that moves between two runs silently changes what the metrics
  describe.
- The mined corpus is not required to be code this project's authors wrote, and
  a corpus written by someone else is preferred for the mined set: the design
  direction warns that authoring queries for one's own repository yields a
  biased benchmark, and an unfamiliar corpus puts the harness in the position
  the agent is actually in.
- The mined corpus is large enough that the reported accuracy is precise enough
  to judge the Phase 1 exit criterion against its target; the achieved label
  count and the resulting confidence interval are reported, so a figure too
  imprecise to support a verdict is visible as such.
- No content from a third-party corpus — source text, commit subjects, or
  symbol names — is committed to this repository. Labels are regenerated from
  the pinned revision.
- Because the mined corpus need not match the languages this project will be
  used on, the documentation-derived and hand-written label sets are drawn from
  repositories that do, and results are reported per corpus as well as in
  aggregate. A single aggregate over a language-skewed corpus would report a
  quality that does not transfer.

### Metrics

- The harness reports top-1, top-3, and top-10 accuracy and mean reciprocal rank
  over the label set, and the definition of a hit is stated precisely.
- Latency is reported as median and 95th percentile of end-to-end query time,
  measured through the same path an agent uses rather than an in-process
  shortcut.
- Peak GPU memory during the run is reported.
- Metrics are reported separately for the ablations the design bets on: lexical
  alone, dense alone, and fused. Without those, an improvement cannot be
  attributed and a redundant component cannot be found.
- Results are broken down by label source and by query length, because a single
  aggregate hides that one source is carrying the score.
- The harness reports the number of labels scored alongside every metric, so a
  figure computed over a handful of labels cannot be mistaken for a stable one.

### Held-out sanity set

- A small hand-written set of natural questions is maintained separately and
  reported separately, and is documented as a sanity check that must never be
  used as the primary metric or tuned against.

### Proxy efficiency measure

- The harness reports, for each label, the bytes of code an agent would read
  before the correct symbol enters context, and compares that against a
  keyword-search baseline over the same repository — the project direction's
  proxy for value delivered while the full agent comparison is deferred.
- The baseline is a real command an agent already has, and the comparison states
  what was run.

### Reproducibility

- A run records the repository and revision, the index revision, the model
  identity, the retrieval configuration, and the harness version, so two runs
  can be compared and a difference attributed.
- Output is machine-readable as well as human-readable, so successive runs can
  be diffed.

## Constraints and non-goals

- No tuning against the mined set within this task. Producing a measurement and
  then fitting to it in the same change destroys the measurement's meaning.
- No full agent A/B comparison. The project direction defers it as confounded,
  slow, and expensive, and schedules it after Phase 3.
- No tool-description benchmark. Whether an agent chooses to call a tool is a
  separate question from whether the tool retrieves well, and the project
  direction keeps the two benchmarks apart.
- No reranking in any measured configuration. The point of this task is to
  produce the un-reranked baseline that Phase 2 must beat.
- No mining of issue trackers or external services. Git history only.
- No hand-labelling to fill gaps where mining is thin. A thin slice is a
  reported fact, not a gap to paper over.
- No swapping the mined corpus for a different one after seeing its numbers.
  Choosing the corpus that scores best is fitting the benchmark to the
  system; the corpus and its revision are fixed before the first measured run.
- No continuous or scheduled evaluation runs. The harness is invoked.

## Acceptance criteria

### Agent-verifiable

1. Mining a fixture repository with a known history produces exactly the
   expected labels, and each filter rule is covered by a commit it rejects and a
   commit it accepts.
2. Rejection counts per rule are reported and sum to the number of commits
   examined minus those accepted.
3. Documentation-derived labels are shown to be excluded from the index for
   their own queries, asserted by a test that would fail if the text were
   indexed.
4. Labels whose expected symbols are absent from the index are discarded and
   counted separately from misses.
5. Metric computation is unit tested against hand-constructed rankings with
   hand-computed top-k accuracy and mean reciprocal rank, including ties and
   the empty-result case.
6. Mining the same history twice produces identical output.
7. A run emits machine-readable output containing repository revision, index
   revision, model identity, retrieval configuration, and harness version.
8. Every reported metric is accompanied by the number of labels it was computed
   over.
9. The full validation suite passes: `./ci.sh`

### Human-verifiable

1. Labels are mined from the pinned mined corpus, and the achieved label count,
   the confidence interval it implies for a top-3 near the 0.80 target, the
   per-rule rejection counts, and a read sample of accepted pairs are reported,
   with the sample judged for whether the pairs are genuinely answerable from
   the code.  
   Command: `cargo run --release -p eval --example mine -- <mined-corpus> --report`
2. A full evaluation run is executed on the target machine and top-1, top-3,
   top-10, mean reciprocal rank, median and 95th percentile latency, and peak
   GPU memory are reported for lexical-only, dense-only, and fused
   configurations.  
   Command: `cargo run --release -p eval --example evaluate -- <store-path> --ablations`
3. The proxy efficiency measure is run against the keyword-search baseline and
   the byte counts for both are reported.  
   Command: `cargo run --release -p eval --example proxy_kpi -- <repo-path> <store-path>`
4. The held-out hand-written question set is run and its results are read and
   judged, separately from the mined metrics.  
   Command: `cargo run --release -p eval --example evaluate -- <store-path> --set handwritten`
5. The documentation-derived set is run over a repository in the languages this
   project will be used on, and its figures are reported beside the mined
   corpus's rather than merged into them, so any gap between the two is
   visible.  
   Command: `cargo run --release -p eval --example evaluate -- <store-path> --set docstring`
