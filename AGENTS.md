# Working in this repository

A local MCP server that uses an RTX 2060 as a code-intelligence coprocessor for
cloud coding agents. **There is no code yet.** Planning is complete; Phase 1 is
decomposed into 13 spec/plan pairs and none have been implemented.

## Your task

You will be assigned a specific task. Work that task and no other.

Each task is a pair: a spec in `docs/development/specs/` saying what and why,
and a plan in `docs/development/plans/` saying how. Read both fully before
touching code. The plan's ordered steps are the sequence to follow — they are a
TDD ratchet, and the "confirm they fail" instructions are load-bearing.

[`docs/development/STATUS.md`](docs/development/STATUS.md) is the authoritative
status of every task; no other file records one. Check there that your assigned
task's dependencies are `DONE` before you start, and say so rather than
proceeding if they are not.

## Rules that are not stylistic

- **One task per branch.** Never two task ids in one branch or one commit.
- **Branch naming:** task id and slug, no suffix — `SIFT-001-workspace-skeleton`.
- **Do not begin the next task.** Finishing means reporting the handoff and
  stopping. Inferring what comes next is not your call.
- **Specs contain no code** — not a signature, not a filename to be created.
  **Plans contain no implementations** — signatures only, bodies elided. If you
  are editing either document, this is the constraint you are editing under.
- **Status lives only in STATUS.md.** Never write a status value into a spec or
  plan.
- **Human-verifiable criteria are not yours to check off.** Every spec splits
  acceptance into agent-verifiable and human-verifiable. The second list needs
  hardware, long runs, or judgement. Report them as outstanding; do not mark a
  task done on their behalf.

## Design authority

`docs/tech-stack.md` governs. Where it disagrees with
`docs/cuda-mcp-rtx2060-plan.md`, it wins.

This matters immediately: the plan document's *Stack* and *Repository structure*
sections describe a Python package. That is superseded. **The runtime is a Rust
workspace and Python appears only in `tools/` for one-time model export**, which
`tech-stack.md` states as a decision and lists "Python runtime" under
*Explicitly rejected*. Do not write runtime Python.

## Structural constraints

- **The workspace must build and test with no GPU, no CUDA toolkit, and no ONNX
  Runtime present.** This is not a preference; CI runs CPU-only.
- **GPU-dependent code stays behind the `inference` crate's trait and its
  non-default `cuda` feature.** No other crate may depend on `ort` or `cudarc`
  directly. `crates/mcp-client` must additionally stay free of any inference,
  tokenizer, or search-index dependency — it is spawned per agent session and
  has a 200 ms cold-start budget.
- **Validation is `./ci.sh`** once SIFT-001 lands. It is the single command, and
  CI runs exactly it — add new checks to the script, never to the workflow.

## Evaluation corpora

Do not substitute a different corpus for the one a task names. The mined label
set comes from a pinned third-party checkout because first-party repositories
here yield too few labels to judge the accuracy target against; the reasoning
and the measured figures are in `docs/development/STATUS.md` and SIFT-012.
Choosing a corpus after seeing its numbers invalidates the Phase 2 gate.
