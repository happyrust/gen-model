# Feature Specification: process-local model pending ledger

**Feature Branch**: `codex/libgm-primitive-caliber`  
**Created**: 2026-08-25  
**Status**: Accepted  
**Decision**: `docs/adr/ADR-050-model-update-pending-is-process-local.md`

## Goal

Ensure a new `aios-database` process never consumes `model_update_pending`
rows created by an earlier process.

## Requirements

1. After the database connection succeeds, startup MUST delete every
   `model_update_pending` row, independent of action, dbnum, or watch scope.
2. Cleanup MUST precede spatial state loading, preload, manager construction,
   watcher startup, worker startup, and every model generation path.
3. Cleanup failure MUST fail startup.
4. Startup output MUST report the number of deleted rows.
5. Current-process watch-scope filtering MUST remain in force.
6. No other recovery/watermark table may be deleted by this feature.

## Acceptance

- A database containing two rows with different dbnums/actions is empty after
  the cleanup helper returns, and the helper reports `2`.
- A source-order regression test proves connection < cleanup < spatial load <
  `run_cli` (which owns manager/watcher/worker/model startup).
- A live probe inserts historical rows, restarts the deployed service, observes
  the cleanup log before model activity, and queries zero remaining rows.
