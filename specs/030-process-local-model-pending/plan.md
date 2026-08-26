# Implementation Plan

## Constitution Check

- Single authority: one startup helper owns table cleanup.
- Fail closed: query/check/decode failures propagate and stop startup.
- Ordering: the call is in `run_app`, after connection and before any producer
  or consumer; a source-order test prevents drift.
- Queue exits: current-process rows retain consume/settle/revive behavior; the
  process boundary has one explicit exit, startup deletion.
- Scope: only `model_update_pending`; watermarks and commit records are untouched.

## Steps

1. Add a database-parameterized cleanup helper and global wrapper.
2. Call it at the first post-connection point in `run_app`; log count and
   propagate failure.
3. Update scope/status wording and ADR-048/Spec-029 cross-restart claims.
4. Add memory-DB behavior and source-order regression tests.
5. Run format, focused tests, check, release build, and live restart probe.

## Rollback Boundary

Restore the four Rust source files and deployed executable from the captured
pre-change copies. Already committed RocksDB data/model state is not altered.
