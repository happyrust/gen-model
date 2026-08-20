# room probe edge-traversal verification

## Changed behavior

- `member_edges_by_room`: `FROM room_relate WHERE out IN [...]` → `FROM pe:<id><-room_relate,...`.
- `rooms_of_element`: `FROM room_relate WHERE out = pe:<id>` → `FROM pe:<id><-room_relate`.
- `room_member_edges`: predicate scans of `room_panel_relate` / `room_relate` → `pe:<room>->room_panel_relate` and `pe:<panel>->room_relate`.

The original join through `tubi_relate.out` was also semantically incompatible with `room_relate.out`: the former points at `inst_geo`, while the latter points at `pe`.

## Exact verification

### Regression test

Command:

```powershell
cargo test --locked --bin node_gen_room_probe 'tests::room_probe_walks_edge_indexes_instead_of_scanning_relation_tables' -- --exact --nocapture
```

Literal result: `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`; exit status `0`. Full output: `unit-test.log`.

### Database plan and timing

Baseline command shape:

```sql
SELECT room_num, count() AS c FROM room_relate WHERE out IN [pe:24384_23257] GROUP BY room_num EXPLAIN;
```

Literal baseline operation: `Iterate Table`.

Modified command:

```sql
SELECT room_num, count() AS c FROM pe:24384_23257<-room_relate GROUP BY room_num EXPLAIN;
```

Literal output: `operation=Iterate Edges`, database time `1.1037ms`; exit status `0`. The direct edge result returned `[]` in `199.5µs`. Source: `output/bran-room-staged/20260820-134513/post-run-query.json`.

### BRAN live probe

Command:

```powershell
$env:RUST_MIN_STACK='33554432'; $env:DB_OPTION_FILE='python/testbed/DbOption-pytest'; cargo run --locked --bin node_gen_room_probe -- '24384/23257'
```

Literal results: `status=Generated`, `model_instance_count=14`, `generated_instance_count=9`, subtree scope `10`, `SUBTREE-ROOMS|24384/23257|scope=10|<no edges>`; exit status `0`. The queue had no requested room work (`ROOM-QUEUE|before_drain=0`), so this fixture verified the fast reporting path rather than a membership change. Full log: `output/bran-room-staged/20260820-134513/node-gen-room-probe-24384_23257.log`.

### Staged increment and restoration

The 210..243 staged window generated BRAN `24384/23257` and advanced watermark 209→243. The release-gate test then failed its `warnings == 0` assertion because this fixture produced four classified warnings; exit status `101`. The batch also reported `room_scope_requested=0`, so it did not exercise room recomputation. Full log: `output/bran-room-staged/20260820-134513/staged-regeneration-stack32m.log`.

Rollback restoration literal result: `applied_sesno=209`, `pe_count=6542`, `port8019_closed=True`; snapshot SHA-256 `D1534C0D4160630FF2E2EE4C9399E8F596341A94363736951FFE3B514805338A`.

## Artifacts and hashes

- Modified artifact: `src/bin/node_gen_room_probe.rs`, SHA-256 `083BCBD23D0E66B6DCE6329CD815FE85DD1F6731BB2645725A2B5B5A8DCEE6A1`.
- Patch: `node_gen_room_probe.patch`.
- Verification: this file plus `unit-test.log`.
- Rollback: `rollback.ps1`.
- Preserved original: `docs/evidence/2026-08-20-room-probe-edge-traversal/node_gen_room_probe.before.rs`, SHA-256 `19BEBEFE731A11251B5E6168B3B0DBC95C6BE63C52C077C93FDDCD75D6F07653`.

Rollback was executed against `docs/evidence/2026-08-20-room-probe-edge-traversal/rollback-sandbox.rs`; literal result: `ROLLBACK_OK`, restored hash matched the original.
