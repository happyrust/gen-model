# Implementation Plan

## Constitution Check

- Single authority: startup work selection remains the watcher/manual shared
  predicates and watermark comparison.
- Fail closed: unreadable identity/session remains blocked by existing gates.
- No second consumer: the existing scheduler and worker execute all discovered work.
- Scope: explicit full-build tools remain unchanged; only service startup loses
  its implicit full-build branch.
- Regression: source-order and forbidden-anchor tests fail if the old branch returns.

## Steps

1. Remove startup derivation of `full_model_requested` from model switches.
2. Configure initialization for increment-produced model work and remove direct
   and deferred `gen_all_geos_data` branches from `run_cli`.
3. Update ordering tests and add a forbidden-anchor regression.
4. Record ADR/spec/changelog changes.
5. Format, test, check, release-build, deploy, and restart with unchanged live data.

## Rollback Boundary

Restore `src/lib.rs`, the prior executable, and documentation from the captured
artifact directory. Database data and watermarks are not changed by this patch.
