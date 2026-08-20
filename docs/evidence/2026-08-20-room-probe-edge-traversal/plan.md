# Plan

1. Replace `room_relate` and `room_panel_relate` predicate scans in `src/bin/node_gen_room_probe.rs` with SurrealDB record-edge traversal.
2. Add a regression test that rejects the old table-scan SQL forms.
3. Verify the unit test, live execution plan, BRAN probe, and runnable rollback.
