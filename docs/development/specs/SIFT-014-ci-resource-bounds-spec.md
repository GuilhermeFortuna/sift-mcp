# SIFT-014: CI resource bounds

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Project direction:** [`../../cuda-mcp-rtx2060-plan.md`](../../cuda-mcp-rtx2060-plan.md), [`../../tech-stack.md`](../../tech-stack.md)  
**Depends on:** SIFT-001  
**Implementation plan:** [`../plans/SIFT-014-ci-resource-bounds-plan.md`](../plans/SIFT-014-ci-resource-bounds-plan.md)

## Purpose

The validation suite can exhaust a developer workstation while compiling and
linking its largest test targets, making the required pre-handoff check unsafe
to run. Limiting compilation to one concurrent job did not prevent a session
from reaching 27.9 GB of memory and 5.5 GB of swap while the daemon integration
artifact was produced. This task gives local validation conservative resource
defaults so a correctness check cannot make the desktop unusable.

## Requirements

### Bounded validation

- Local validation uses conservative defaults for both build concurrency and
  test concurrency.
- Debug information generated only for validation is reduced enough that the
  largest test links without exhausting a 32 GB workstation.
- The conservative defaults apply regardless of generic continuous-integration
  environment markers, because those markers do not describe the host's memory
  capacity.
- A developer or dedicated runner can explicitly override each default when
  the host has measured headroom.

### Existing contract

- Validation still runs formatting, strict linting of all targets, the complete
  workspace test suite, and a release build in the established order.
- Default validation remains CPU-only and does not enable GPU features.
- The resource policy is protected by a durable regression test that fails if
  an unsafe automatic parallelism path is restored.

## Constraints and non-goals

- No daemon behavior, protocol, lifecycle, or integration-test semantics are
  changed; the failure occurs while producing validation artifacts.
- No machine-wide memory, swap, kernel, or service configuration is changed.
- No claim is made that one fixed setting is optimal for every host; explicit
  overrides remain the mechanism for measured tuning.
- No Phase 2 feature work or Phase 1 acceptance measurement is included.

## Acceptance criteria

### Agent-verifiable

1. The validation entrypoint defaults build and test concurrency to one and
   never derives either value from processor count.
2. Development and test artifacts built by the validation entrypoint omit full
   debug information unless explicitly overridden.
3. Explicit build, test, and debug-information overrides are preserved.
4. A regression test parses the validation policy and rejects the previous
   unsafe automatic-parallelism behavior.
5. The daemon integration suite passes with the conservative defaults.
6. The full validation suite passes.

### Human-verifiable

1. From a cold build cache during ordinary desktop use, the full validation run
   completes without making the desktop unresponsive; peak resident memory and
   swap remain below the host's capacity.  
   Command: `sift_ci_target=$(mktemp -d); CARGO_TARGET_DIR="$sift_ci_target" /usr/bin/time -v ./ci.sh`
