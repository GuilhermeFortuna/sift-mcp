# SIFT-014 implementation plan: CI resource bounds

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/SIFT-014-ci-resource-bounds-spec.md`](../specs/SIFT-014-ci-resource-bounds-spec.md)  
**Depends on:** SIFT-001

## Current-system context

`ci.sh` is the single validation entrypoint established by SIFT-001. It already
defaults `CARGO_BUILD_JOBS` to one for local runs, but changes that default to
`nproc` whenever `CI=true`; it does not bound Rust test threads or reduce debug
information for development and test artifacts. `tests/ci_sh.rs` checks for the
literal one-job assignment but does not exercise the environment-dependent
branch or the other resource controls.

The most recent affected session reached a 27.9 GB user-slice memory peak and
5.5 GB swap peak. At the same time, the daemon integration executable was about
281 MB and was emitted alongside other large daemon targets. The gap is a
resource policy that controls artifact size and execution concurrency, not only
Cargo's compile job count.

## Interfaces produced

This task adds no library or runtime interface. It changes only the validation
entrypoint's environment defaults and the test that guards them.

## Implementation decisions

- Build concurrency defaults to one on every host because processor count is
  not evidence of available memory, while an explicit `CARGO_BUILD_JOBS`
  remains authoritative for runners that have measured headroom.
- Rust test concurrency also defaults to one because integration cases create
  independent stores and indexes; serial execution bounds their aggregate
  resident set without changing any test's behavior.
- Development and test debug information default to disabled within validation
  because symbol-rich daemon test executables are the large link inputs, while
  validation requires diagnostics and backtraces rather than debugger-ready
  artifacts.
- The debug settings remain environment defaults rather than manifest profile
  changes because ordinary `cargo build` and developer debugging are outside
  the validation entrypoint's resource policy.
- The regression test checks complete policy assignments and rejects processor-
  count derivation because substring checks alone allowed an unsafe conditional
  branch to coexist with the safe-looking local default.

## Ordered implementation

1. Create the branch `SIFT-014-ci-resource-bounds`.
2. Add the SIFT-014 spec/plan pair and register it as a Batch 01 corrective task
   in the status table. Commit.
3. Replace the existing CI-script regression with failing assertions that
   require one-job build concurrency, one-thread test concurrency, reduced
   development and test debug information, preservation of explicit overrides,
   and no `nproc` branch. Run it and confirm it fails. Implement the validation
   defaults, confirm the test passes, and commit.
4. Run the daemon integration suite with the conservative environment defaults
   and record elapsed time and peak resident memory.
5. Run the full linting, formatting, workspace tests, and release build through
   `./ci.sh`. Confirm the entire suite passes and leave the task ready pending
   human cold-cache verification.
6. Human step: run the full suite with a fresh temporary build directory during
   ordinary desktop use, record peak resident memory and swap, then update the
   task status and commit.

## Validation

- **Unit:** the validation-policy regression checks every conservative default,
  each explicit override, and the absence of processor-count inference.
- **Integration:** the daemon integration test executable is built and run with
  the policy applied.
- **Regression:** the established formatting, strict lint, full test, and
  release-build stages remain present and ordered.
- **Manual:** a cold-cache full run remains responsive on the target workstation.
- **Measurement:** peak resident memory from the isolated daemon suite is
  reported; the human cold-cache run records whole-suite peak memory and swap.

```bash
cargo test --test ci_sh
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 /usr/bin/time -v cargo test -p daemon --test daemon_integration
./ci.sh
sift_ci_target=$(mktemp -d); CARGO_TARGET_DIR="$sift_ci_target" /usr/bin/time -v ./ci.sh
```

## Handoff

Report the defaults and their override variables; the prior session's 27.9 GB
memory and 5.5 GB swap evidence; the daemon integration suite's test count,
elapsed time, and peak resident memory under the new policy; the full validation
result; and the outstanding cold-cache human measurement.
