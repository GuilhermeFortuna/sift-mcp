# SIFT-013: Phase 1 acceptance and locked baseline

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Project direction:** [`../../cuda-mcp-rtx2060-plan.md`](../../cuda-mcp-rtx2060-plan.md), [`../../tech-stack.md`](../../tech-stack.md)  
**Depends on:** SIFT-011, SIFT-012  
**Implementation plan:** [`../plans/SIFT-013-phase-1-acceptance-plan.md`](../plans/SIFT-013-phase-1-acceptance-plan.md)

## Purpose

Every component of Phase 1 has been measured on its own, but the exit criteria
are stated for the assembled system: top-3 accuracy at or above 0.80, search
latency at the 95th percentile under 400 ms end to end, and cold agent start
under 200 ms. Nobody has yet run all three through the path an agent actually
uses. This task produces that measurement, records it as an acceptance record,
and locks the resulting figures as the baseline. That baseline is the whole
mechanism of the Phase 2 gate: reranking is kept only if it improves top-1 by a
material margin, and without a locked, reproducible number there is nothing to
compare against.

## Requirements

### Measurement conditions

- Every measurement is taken through the full path an agent uses — the spawned
  client, the socket, the daemon — rather than through an in-process shortcut,
  because the SLO is stated end to end.
- Measurements are taken on the target hardware with a desktop session attached,
  matching the operating conditions the VRAM budget assumes.
- The corpus used is stated by name and revision, is within the size range the
  design targets, and is large enough that the figures mean something at scale.
- Accuracy is measured over the mined corpus fixed in SIFT-012 and not over a
  corpus chosen after seeing results, and the label count and the confidence
  interval it implies are reported beside the accuracy figure. A top-3 whose
  interval spans the target supports no verdict, and that is recorded as the
  outcome rather than resolved by rounding.
- Figures for the first-party documentation-derived set are reported beside the
  mined corpus's, since the mined corpus's languages differ from those this
  project will be used on and a single aggregate would overstate transfer.
- Every measurement records the revision of the code, the index, the model, and
  the configuration that produced it, and enough of the environment that a
  disagreeing rerun can be diagnosed.
- Latency figures come from a stated number of runs after a stated warm-up, and
  both median and 95th percentile are reported; a single run is not a
  measurement.

### The three exit criteria

- Top-3 accuracy over the mined label set is reported against the 0.80 target,
  with the label count alongside it.
- End-to-end search latency at the 95th percentile is reported against the
  400 ms target, with the per-stage split, so a miss can be attributed to a
  stage rather than to the system.
- Cold agent start is reported against the 200 ms target, measured as an agent
  experiences it with the daemon already warm, and separately with the daemon
  cold so the difference is visible.
- Peak GPU memory under the full workload is reported against the usable budget.
- Each criterion is judged met or not met explicitly. A criterion that is missed
  is recorded as missed, with the measured value, and not rounded toward the
  target.

### Baseline and gate

- The measured figures are recorded as a locked baseline, stored so that a later
  run can be compared against them mechanically rather than by memory.
- The baseline includes the per-ablation figures, so Phase 2's gate has both the
  fused number to beat and the evidence of what each retrieval path contributes.
- The record states the margin by which Phase 2 reranking would have to improve
  top-1 to be kept, and that this is the criterion the project direction sets.
- The acceptance record lists every human-verifiable criterion across the batch
  with its measured evidence, so the batch's completeness is checkable in one
  place.

### Outcome

- If a criterion is not met, the record states what was measured, which stage or
  component the evidence points to, and what would have to change — as findings,
  not as work performed.
- The status of the batch reflects the measured outcome rather than the
  intention.

## Constraints and non-goals

- No optimization. This task measures and records; it does not tune. Fixing a
  missed criterion in the same change that measures it destroys the record's
  credibility and is the single strongest temptation here. A remedy identified
  by this task becomes its own task.
- No reranking, no cross-encoder, no Phase 2 work of any kind. The gate cannot
  be evaluated by the change that opens it.
- No new features, no new tools, no changes to the tool surface.
- No full agent A/B comparison. The project direction schedules that after
  Phase 3; the proxy measure from SIFT-012 stands in for it here.
- No changes to the evaluation harness's metrics or label mining. If the harness
  is wrong, that is a defect against SIFT-012, not a redefinition made while
  measuring.
- No comparison against other retrieval products or published benchmarks.

## Acceptance criteria

### Agent-verifiable

1. The locked baseline is committed in a machine-readable form containing every
   reported figure, its label or run count, and the revisions and configuration
   that produced it.
2. A comparison routine reads the locked baseline and a fresh result and reports
   per-metric deltas, verified against a constructed pair of inputs.
3. The acceptance record enumerates every human-verifiable criterion from
   SIFT-001 through SIFT-012 and pairs each with its evidence or an explicit
   note that it is outstanding.
4. Each of the three exit criteria appears in the record with a measured value
   and an explicit met-or-not-met judgement.
5. The status table reflects the outcome, and no task is marked complete whose
   human-verifiable criteria lack recorded evidence.
6. The full validation suite passes: `./ci.sh`

### Human-verifiable

1. Top-3 accuracy over the mined label set is measured through the agent-facing
   path and reported against the 0.80 target, with the label count and the
   confidence interval it implies, and with the corpus named by revision.  
   Command: `scripts/acceptance/measure-accuracy.sh <mined-corpus> <store-path>`
2. End-to-end search latency is measured over at least 200 queries after warm-up
   through the spawned client and socket, and median, 95th percentile, and the
   per-stage split are reported against the 400 ms target.  
   Command: `scripts/acceptance/measure-latency.sh <store-path> --queries 200`
3. Cold agent start is measured over at least 20 runs with the daemon warm, and
   separately with the daemon cold, and both are reported against the 200 ms
   target.  
   Command: `scripts/acceptance/measure-cold-start.sh`
4. Peak GPU memory under a concurrent search-and-index workload is measured with
   a desktop session attached and reported against the ~5.0 GB usable budget.  
   Command: `scripts/acceptance/measure-vram.sh <repo-path> <store-path>`
5. The three ablation configurations are measured through the same path, and the
   contribution of each retrieval path is read and judged for whether both earn
   their place.  
   Command: `scripts/acceptance/measure-ablations.sh <store-path>`
