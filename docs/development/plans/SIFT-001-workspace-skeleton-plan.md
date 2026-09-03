# SIFT-001 implementation plan: Workspace skeleton and validation suite

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/SIFT-001-workspace-skeleton-spec.md`](../specs/SIFT-001-workspace-skeleton-spec.md)  
**Depends on:** None

## Current-system context

The repository contains untracked planning documents and no commits:
`docs/cuda-mcp-rtx2060-plan.md`, `docs/tech-stack.md`, the task documents under
`docs/development/`, and `AGENTS.md`, which already carries the workflow rules,
the design-authority ruling, and the structural constraints for anyone working
here. There is no `Cargo.toml`, no `.gitignore`, no CI configuration, and no
toolchain pin. Nothing is reusable because nothing exists.

The two documents disagree about language: the plan document's *Stack* and
*Repository structure* sections describe a Python package, while `tech-stack.md`
states "**Decision: Rust workspace**" and lists "Python runtime" under
*Explicitly rejected*. `tech-stack.md` governs, and its crate layout — eight
crates plus a non-runtime `tools/` directory — is the layout this task creates.
The gap this task closes is that no later task has anywhere to put a crate, and
no definition of "green" to be held to.

## Interfaces produced

This task adds no library surface. It produces configuration and one test that
guards the workspace shape.

```toml
# Cargo.toml (workspace root)
[workspace]
members = [
  "crates/mcp-client",
  "crates/daemon",
  "crates/indexing",
  "crates/retrieval",
  "crates/inference",
  "crates/storage",
  "crates/change-intel",
  "crates/eval",
]
resolver = "3"

[workspace.package]
edition = "2024"

[workspace.dependencies]
# Declared once here, inherited by members with `workspace = true`.
# This task declares only what a member actually uses; it is not a pre-wiring list.
```

```toml
# rust-toolchain.toml
[toolchain]
channel = "<pinned stable version supporting edition 2024>"
components = ["rustfmt", "clippy"]
```

```rust
// tests/workspace_shape.rs  (workspace-root integration test)
/// Parses the root manifest and the `crates/` directory and asserts the two
/// agree: every directory is a declared member and every member exists.
#[test]
fn every_crate_directory_is_a_declared_member() { /* elided */ }

/// Asserts the member list equals the crate layout recorded in tech-stack.md,
/// so adding a crate without updating the architecture document fails here.
#[test]
fn member_list_matches_documented_layout() { /* elided */ }
```

```bash
# ci.sh
# The single validation command. Exits non-zero if any stage fails.
```

## Implementation decisions

- **Eight crates are created empty rather than growing organically.** The
  alternative — adding a crate when its task starts — means the dependency
  direction between crates is decided eight separate times, under deadline, by
  whoever is mid-task. Creating them now fixes the architecture the tech stack
  document already decided.

- **`inference` is depended on only through a trait defined in `inference`
  itself, and the `ort` dependency sits behind a non-default `cuda` feature.**
  If `retrieval` or `eval` depended on the GPU runtime directly, the workspace
  would stop building on any machine without CUDA and CI would need a GPU
  runner. The feature is off by default so that the default build is the CPU
  build, because the default is what people actually run.

- **Dependency versions live in `[workspace.dependencies]` and members inherit
  with `workspace = true`.** Two crates resolving different versions of
  `tokio` or `serde` produces link-time and trait-coherence errors that read as
  unrelated bugs; one table makes that impossible.

- **The toolchain is pinned in `rust-toolchain.toml` rather than documented in
  prose.** Edition 2024 needs a recent stable compiler; a contributor on an
  older toolchain otherwise gets a parse error pointing at a syntax feature
  rather than at their toolchain.

- **`ci.sh` is the single validation command, and the CI workflow calls it with
  no additional steps.** A workflow that lists its own steps drifts from the
  local command, and the first symptom is a green local run followed by a red
  CI run that nobody can reproduce. New checks are added to `ci.sh`, never to
  the workflow.

- **`ci.sh` runs `cargo clippy --workspace --all-targets -- -D warnings` with
  default features, not `--all-features`.** `--all-features` would enable
  `cuda` and require an ONNX Runtime binary on a CPU-only runner. GPU lint and
  test coverage is a separate, locally-run command documented in
  `CONTRIBUTING.md`.

- **GPU tests are marked `#[ignore]` rather than gated behind a feature.**
  A feature gate hides the test's existence from `cargo test --list`; `#[ignore]`
  leaves it visible and one flag away, so a developer with hardware discovers it.

- **The workspace-shape test reads the layout from `docs/tech-stack.md` rather
  than duplicating the list in Rust.** A hard-coded list in the test is a third
  copy of the architecture, and the third copy is the one that goes stale.

- **`tools/` gets a README and no Python packaging.** SIFT-004 declares its own
  dependencies; creating a `pyproject.toml` now would invite a Python dependency
  before anything needs one, and the tech stack document is explicit that Python
  is confined to one-time tooling.

- **`.gitignore` excludes `target/`, `models/`, `*.onnx`, index and store
  directories, and the ONNX Runtime download cache.** A committed model file is
  a repository nobody can clone; the exclusions go in before the first commit
  because removing a large blob afterwards means rewriting history.

- **The first commit includes the three planning documents.** They are currently
  untracked, and the task documents reference them by path; committing them with
  the skeleton makes those links resolve from the first commit onward.

## Ordered implementation

1. Create the branch `SIFT-001-workspace-skeleton`.
2. Write `.gitignore` covering build output, model artifacts, index and store
   data, and the runtime download cache. Create representative files in each
   ignored location and confirm `git status --porcelain` is empty. Commit,
   including the existing `docs/` tree and `AGENTS.md`, as the repository's
   first commit.
3. Write the root `Cargo.toml` with the eight members, `resolver = "3"`,
   `[workspace.package]` carrying edition 2024, and an empty
   `[workspace.dependencies]`. Write `rust-toolchain.toml` pinning the channel
   and the `rustfmt` and `clippy` components. Create the eight crate
   directories, each with a manifest inheriting the workspace package fields and
   an empty library root. Confirm `cargo build --workspace` succeeds. Commit.
4. Write `tests/workspace_shape.rs` with the two tests named in *Interfaces
   produced*: one asserting every directory under `crates/` is a declared member
   and every member directory exists, one asserting the member list equals the
   layout parsed from `docs/tech-stack.md`. Run them against a deliberately
   missing member and confirm they fail with a message naming the crate. Restore
   the member and confirm they pass. Commit.
5. Add the `cuda` feature to `inference`, defaulting off, with `ort` as an
   optional dependency activated by it. Add an `#[ignore]`-marked placeholder
   test in `inference` asserting it is only meaningful with hardware. Confirm
   `cargo test --workspace` runs and skips it, and that
   `cargo build --workspace` succeeds with no ONNX Runtime present. Commit.
6. Write `ci.sh` running, in order: `cargo fmt --all -- --check`,
   `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`, `cargo build --workspace --release`. Make it exit
   non-zero on the first failure. Introduce a deliberate lint warning, confirm
   `./ci.sh` fails and names it, remove it, confirm `./ci.sh` passes. Commit.
7. Add a GitHub Actions workflow on a CPU-only runner whose only build step is
   `./ci.sh`, with a Rust toolchain step honouring `rust-toolchain.toml` and a
   cargo cache. Commit.
8. Write `CONTRIBUTING.md` covering what `AGENTS.md` does not: local setup, the
   eight crates and the purpose of each, the separate local command for running
   GPU tests, and how to obtain the CUDA toolkit and ONNX Runtime. Reference
   `AGENTS.md` for the workflow rules, the branch-naming convention, the
   design-authority ruling, and the GPU-behind-the-trait constraint rather than
   restating them — two copies of a rule is one copy and one stale copy. Add a
   test asserting `CONTRIBUTING.md` does not redefine the validation command,
   so the single-source rule survives future edits. Commit.
9. Human step: from a clean clone in a temporary directory, follow only
   `AGENTS.md` and `CONTRIBUTING.md` and reach a green `./ci.sh`, noting any
   point at which a question had to be asked.
10. Human step: run `cargo build --workspace --release`, confirm it completes,
    and confirm the produced binaries start and exit cleanly.
11. Run the full validation suite and confirm it passes.

## Validation

- **Unit:** the two workspace-shape tests, each exercised against a deliberately
  broken manifest before being accepted.
- **Integration:** `./ci.sh` on a clean checkout with no CUDA toolkit and no
  ONNX Runtime present.
- **Regression:** none — this task establishes the baseline rather than
  preserving one.
- **Manual:** a clean-clone walkthrough following only `AGENTS.md` and
  `CONTRIBUTING.md`; correct means a green run with no questions asked.
- **Measurement:** wall-clock duration of `./ci.sh` on a clean checkout and on a
  warm cache, reported once as the cost every later task pays.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
./ci.sh
git clone . /tmp/sift-clean && cd /tmp/sift-clean && ./ci.sh
```

## Handoff

Report the eight crates as created and the workspace-shape test passing; the
pinned toolchain channel and edition; confirmation that `cargo build --workspace`
succeeds with no CUDA toolkit and no ONNX Runtime present, and that
`cargo test --workspace` skips the GPU-marked test; the exact stages `ci.sh`
runs and the evidence that a deliberate lint warning failed it; wall-clock
duration of `./ci.sh` cold and warm; confirmation that CI's only build step is
`./ci.sh`; and the result of the clean-clone walkthrough, including any point at
which `AGENTS.md` or `CONTRIBUTING.md` was insufficient.
