# Implementation Plan: watch scope durable drain

## Constitution Check

- **One authority**: reuse `data_interface::watch_scope::dbnums`; do not read
  `DbOption.toml` directly in the queue module.
- **Fail visible**: scope is rendered into the SQL used by both execution and
  readiness/status; no post-load silent `continue`.
- **Durability**: excluded work is never mutated.
- **Single consumer**: retain ADR-011's existing worker and drain order.
- **Regression proof**: pure SQL-renderer tests fail if the predicate is
  removed from any automatic path.

## Dependency Order

1. Add one pure renderer for the automatic durable-work scope.
2. Inject it into drain, retryable/dead probes, and health/readiness status.
3. Keep exact-key post-commit drains and inspection APIs unchanged.
4. Verify static SQL shape, focused unit tests, and crate compilation.
5. Deploy the rebuilt executable and verify that `dbnum=1112` counts remain
   unchanged while `dbnum=8000` work can advance.

## Rollback Boundary

The behavior change is confined to `model_update_pending` automatic SELECT
rendering plus ADR/spec/changelog. Restoring the preserved original source and
rebuilding returns to global backlog consumption.

