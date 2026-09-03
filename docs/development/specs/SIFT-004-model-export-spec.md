# SIFT-004: Model export and reference fixture

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Project direction:** [`../../cuda-mcp-rtx2060-plan.md`](../../cuda-mcp-rtx2060-plan.md), [`../../tech-stack.md`](../../tech-stack.md)  
**Depends on:** SIFT-001  
**Implementation plan:** [`../plans/SIFT-004-model-export-plan.md`](../plans/SIFT-004-model-export-plan.md)

## Purpose

The runtime carries no Python and no deep-learning framework, so the embedding
model has to arrive as an exported graph in half precision, produced once by
tooling that is never shipped. The risk is silent numerical divergence: an
export that loads, runs, and returns plausible vectors that are subtly wrong
produces a retrieval system that is merely mediocre, with nothing failing to
point at the cause. This task performs the export and — more importantly —
commits a reference fixture of inputs and their known-correct vectors, so that
SIFT-005 has an oracle rather than an opinion. The tech stack document warns
that this export may fight back and budgets a day for it; the fallback model
exists for that reason.

## Requirements

### Export

- The primary embedding model named in the tech stack is exported to a
  framework-independent graph in half precision, and the fallback model is
  exported by the same tooling with no code path unique to either.
- The export is reproducible: the same script, model revision, and options
  produce a functionally identical graph, and the model revision is pinned
  rather than tracking a moving tag.
- The exported graph accepts a batch of variable-length token sequences with
  padding and returns per-token hidden states, so that pooling remains the
  runtime's responsibility and is not baked in.
- The tokenizer files that belong to the exported model are captured alongside
  it, because a tokenizer mismatch is indistinguishable from a bad export at the
  vector level.
- The export records the model identity, revision, embedding width, maximum
  sequence length, the pooling strategy the model expects, and the instruction
  prefix convention if the model has one.

### Correctness evidence

- A fixture pins a fixed set of input strings — code, prose, identifiers, an
  empty string, a string long enough to be truncated, and non-ASCII text — to
  the vectors the original framework implementation produces for them.
- The fixture records the tolerance within which a correct implementation must
  reproduce those vectors, and the reasoning for that tolerance, so that a later
  failure is judged against a stated bar rather than a guess.
- The fixture pins token sequences as well as vectors, so that a tokenizer fault
  and a model fault are distinguishable.
- The exported graph is verified against the fixture by the tooling itself,
  independently of any runtime code, so an export can be rejected before the
  runtime ever sees it.

### Operational constraints

- The exported model's memory footprint at half precision is measured and
  reported, and checked against the stated VRAM budget with a desktop session
  attached rather than on an idle headless device.
- The tooling states its own dependencies and does not add any dependency to the
  runtime workspace.
- The tooling is documented well enough that the export can be repeated by
  someone who did not write it, including which model revision was used.
- Large exported artifacts are not committed to version control; their expected
  location, their content hashes, and how to obtain or regenerate them are.

## Constraints and non-goals

- No runtime inference code. Nothing here is loaded by the daemon or linked into
  any crate. Writing "a small Rust harness to check the export while I'm here"
  is the temptation this rules out — that harness is SIFT-005 and it must be
  written against the fixture, not against a fresh opinion of what is correct.
- No reranker export. Reranking is Phase 2 and is explicitly gated on Phase 1
  measurements; exporting the reranker now would spend the budgeted day on work
  that may be dropped.
- No quantization below half precision, no graph surgery, no operator fusion, no
  provider-specific compilation. Those are optimizations to consider only after
  profiling shows a need.
- No model training, fine-tuning, or distillation.
- No automated model download at runtime or at build time. Obtaining the model
  is a documented, deliberate step.
- No benchmarking of retrieval quality between the primary and fallback models.
  That comparison needs the evaluation harness and belongs to SIFT-012.

## Acceptance criteria

### Agent-verifiable

1. The export tooling declares pinned dependencies and a pinned model revision,
   and refuses to run against an unpinned or mismatched revision.
2. The fixture is committed, is readable without any framework installed, and
   contains for every input string both its token sequence and its reference
   vector.
3. The fixture covers, at minimum: a code snippet, a prose sentence, a bare
   identifier, an empty string, an input longer than the maximum sequence
   length, and non-ASCII text.
4. The recorded metadata — model identity, revision, embedding width, maximum
   sequence length, pooling strategy, instruction prefix convention — is present
   and is machine-readable.
5. Exported artifacts are excluded from version control, and their expected
   location and content hashes are recorded.
6. The full validation suite passes: `./ci.sh`

### Human-verifiable

1. The primary model is exported on the target machine and the tooling's own
   verification against the framework implementation passes within the stated
   tolerance.  
   Command: `python tools/export_model.py --model primary && python tools/verify_export.py --model primary`
2. The fallback model is exported and verified by the same commands, confirming
   the fallback path is real and not theoretical.  
   Command: `python tools/export_model.py --model fallback && python tools/verify_export.py --model fallback`
3. Peak GPU memory during a representative batch is measured with a desktop
   session attached and reported against the ~5.0 GB usable budget.  
   Command: `python tools/verify_export.py --model primary --report-vram`
