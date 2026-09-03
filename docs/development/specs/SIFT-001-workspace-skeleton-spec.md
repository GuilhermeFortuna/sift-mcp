# SIFT-001: Workspace skeleton and validation suite

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Project direction:** [`../../cuda-mcp-rtx2060-plan.md`](../../cuda-mcp-rtx2060-plan.md), [`../../tech-stack.md`](../../tech-stack.md)  
**Depends on:** None  
**Implementation plan:** [`../plans/SIFT-001-workspace-skeleton-plan.md`](../plans/SIFT-001-workspace-skeleton-plan.md)

## Purpose

The repository has no code, no build configuration, and no commits — only three
planning documents. Every later task needs somewhere to put its crate and a
definition of "green" it can be held to, and if those are invented per task they
will diverge: one task will lint, the next will not, and the first CPU-only CI
run will fail on a GPU dependency nobody meant to make mandatory. This task
creates the workspace with every component of the architecture present as an
empty compiling crate, and defines the single validation command that every
subsequent task must pass. It matters now because it is the only task whose
output every other task in the batch depends on.

## Requirements

### Workspace shape

- The workspace contains exactly one crate per component named in the tech stack
  document's crate layout, with no component missing and none added.
- Every crate compiles, is reachable from the workspace root, and is empty of
  behaviour beyond what is needed to compile.
- Dependency versions are declared once for the workspace and inherited by
  member crates, so two crates can never resolve different versions of the same
  library.
- The Rust edition and a minimum toolchain version are pinned in the repository,
  so a contributor with a different local toolchain gets a clear error rather
  than a subtly different build.

### Separation of GPU-dependent code

- Code requiring a CUDA toolkit or an ONNX Runtime binary is confined to crates
  that other crates depend on only through an abstraction, never directly.
- The entire workspace builds and its tests pass on a machine with no GPU, no
  CUDA toolkit, and no ONNX Runtime installed.
- GPU-requiring tests are marked such that the default test run skips them, and
  are runnable on demand by a developer with the hardware.

### Validation suite

- A single documented command runs formatting checks, linting, the test suite,
  and a release build, and fails if any of them fail.
- Lint warnings fail the command; a warning that does not fail is a warning that
  accumulates.
- The same command is what continuous integration runs, so a green local run and
  a green CI run cannot disagree about what was checked.
- Continuous integration runs on a CPU-only runner and completes without access
  to a GPU.

### Repository hygiene

- Build output, model files, index data, and any downloaded runtime binaries are
  excluded from version control.
- The repository has an initial commit; the planning documents already present
  are committed as part of it rather than left untracked.
- A contributor-facing document states the validation command, the crate layout
  and what each crate is for, and the rule that GPU code stays behind the
  abstraction.

## Constraints and non-goals

- No behaviour. Not a chunker, not a parser, not a database call. Crates are
  empty apart from what compilation requires. The temptation to "just start the
  storage schema while I'm here" is exactly what this non-goal exists to stop.
- No dependency is added to a crate that does not yet need it. A workspace-wide
  version table is not permission to pre-wire libraries into crates; each later
  task adds its own.
- No model download, no ONNX export, no CUDA verification. That is SIFT-004.
- No MCP protocol work, no socket, no daemon lifecycle. Those are SIFT-010 and
  SIFT-011.
- No release automation, publishing, packaging, or cross-compilation. Single
  local release build only.
- No Python packaging or environment management for the tooling directory. The
  directory exists and is documented as non-runtime; SIFT-004 gives it contents
  and its own dependency declaration.

## Acceptance criteria

### Agent-verifiable

1. Every crate named in the tech stack document's layout exists in the workspace
   and is listed as a workspace member; a test or check fails if the two lists
   disagree.
2. A clean checkout builds with no network access to a GPU runtime and no CUDA
   toolkit present.
3. Introducing a deliberate lint warning causes the validation command to fail,
   and removing it causes the command to pass.
4. The continuous integration configuration invokes the same validation command
   a developer runs locally, not a separately maintained list of steps.
5. Version control excludes build output, model files, and index data: creating
   representative files in those locations leaves the working tree clean.
6. The full validation suite passes.

### Human-verifiable

1. The release build completes on the target machine and the resulting binaries
   are confirmed to start and exit cleanly.  
   Command: `cargo build --workspace --release && ls -la target/release`
2. A developer following only the contributor-facing document, from a clean
   clone, reaches a green validation run without asking a question.  
   Command: `git clone <this repo> /tmp/sift-clean && cd /tmp/sift-clean && ./ci.sh`
