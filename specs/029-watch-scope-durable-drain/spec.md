# Feature Specification: watch scope durable drain

**Feature Branch**: `codex/libgm-primitive-caliber`  
**Created**: 2026-08-25  
**Status**: Accepted  
**Decision**: `docs/adr/ADR-048-watch-scope-arms-startup-work.md`

## Goal

When an effective `watch_dbnums` / `--watch-dbnum` list is present, the
process may automatically consume current-process model work only for rows
whose stored `dbnum` is in that list. Cross-restart behavior is governed by
ADR-050: every row left by a previous process is cleared at startup.

## Requirements

1. The global automatic drain for data/model and room work MUST apply the
   effective `watch_scope` to `model_update_pending.dbnum`.
2. Retryable, dead-letter, and model-readiness queries MUST use the same
   predicate as the automatic drain, so excluded rows neither execute nor
   keep the scoped run unready.
3. An empty watch scope MUST preserve the historical unrestricted behavior.
4. Exact post-commit scoped work from the currently admitted batch MUST keep
   using its exact task keys; it MUST NOT be widened to unrelated rows.
5. Current-process excluded rows MUST NOT be failed, have `attempts`
   incremented, or have `revision` changed merely because this process skipped
   them. They are discarded with the rest of the table on the next startup.
6. Read-only pending-unit inspection MUST continue to expose all rows from the
   current process.
7. Rows without a usable `dbnum` are outside every non-empty numeric watch
   scope. They remain inspectable only for the lifetime of the current process.

## Acceptance

- With scope `[8000]`, generated automatic SELECTs include
  `(dbnum?:0) IN [8000]` and cannot select rows for `1112` or `0`.
- With no scope, generated SELECTs contain no dbnum predicate.
- Drain, retryable/dead status, and readiness renderers share one predicate.
- Existing exact-key scoped drain queries remain unchanged.
- Focused Rust tests and `cargo check` pass.
