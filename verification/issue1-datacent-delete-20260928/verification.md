# plant-ui Issue #1 verification (2026-09-28)

## Verdict

Supported on branch `codex/model-projection-cache` and included in `v0.1.31`.

The first live run below was against the older `0.1.30` service and established the
failure baseline: E3D produced the rename/delete sessions, but that runtime did not
maintain the table. The branch implementation added the `datacenter_delete` ledger
in commit `1e637732f` and subsequent fixes. A later E3D fixture run against that
implementation passed all 10 scenarios; its `delete-unit` case wrote the deleted
EQUI name, refno, nearest ZONE, session, timestamp, and `reason = deleted`.

## Runtime under test

- Service: `http://127.0.0.1:8022`
- Version: `0.1.30`
- Build: `0.1.30+g4c305d820e64.1790227581.dirty`
- Project/MDB: `AvevaMarineSample` / `/ALL`
- Watched dbnum: `8000`
- Initial dbnum 8000 session: `640`
- E3D driver: `D:/Rust/target/debug/l3_suite.exe`
- Native database: `D:/AVEVA/Projects/E3D3.1/AvevaMarineSample/ams000/ams8000_0001`

## E3D TTY scenario

The scenario used one scratch BRAN copied from `/C-OR-1R345-C` under
`/1RX-345`. Each stage contained exactly one `SAVEWORK`.

| Stage | Session | Observation |
| --- | --- | --- |
| Create | 640 -> 642 | Created `/CODEX_I1_20260928_A`, refno `24384/26496` |
| Rename | 642 -> 644 | Same refno changed from `/CODEX_I1_20260928_A` to `/CODEX_I1_20260928_B` |
| Delete | 644 -> 646 | Deleted `/CODEX_I1_20260928_B` |

All three driver outcomes are `completed`. The running service reached dbnum
8000 session `646`, generation `82`. A final request to
`GET /api/v1/tree/ancestors?refno=24384/26496` returned HTTP 404 with
`dbnum 8000 ... index ... not found`, confirming that no live scratch element
was left behind.

## Branch implementation

- `BRAN`, `SUPPO`, and `EQUI` delivery-unit deletion writes the pre-delete name.
- A delivery-unit `NAME` change writes the old name; the current element continues
  through the normal update path.
- Child components do not create delete-ledger rows.
- The key is `[refno, old_name]`, so replay is idempotent and multiple historical
  names for one refno remain distinct.
- The row also stores `dbnum`, `sesno`, `sesno_time`, and `reason`; missing ZONE
  ancestry is stored as `NONE`.
- `rs-server` is outside this verification scope and was left unchanged.

Focused verification on `codex/model-projection-cache`:

- `cargo test --lib data_interface::increment_pipeline::datacenter_tests:: -- --test-threads=1`:
  19 passed.
- `cargo test --lib datacenter_delete`: 5 passed.
- E3D fixture report: 10/10 passed, including `delete-unit` and its
  `datacenter_delete` assertion.

## Evidence

- `D:/work/plant-code/old/gen-model-model-cache/output/e3d-fixture/20260924-130105/report.md`
- `D:/work/plant-code/old/gen-model-model-cache/output/e3d-fixture/20260924-130105/delete-unit/assertions.json`
- `D:/work/plant-code/old/gen-model-model-cache/output/e3d-fixture/20260924-130105/delete-unit/datacenter-delete-apply.json`
- `create-driver/check-driver-evidence.json`
- `rename-driver/check-driver-evidence.json`
- `delete-driver/check-driver-evidence.json`
- `create.log`, `rename.log`, `delete.log`
- `create.mac`, `rename.mac`, `delete.mac`

The normal Rust test command could not start because Cargo's build-script file
scan hit an existing access-denied directory at `python/.pytest_cache`. This did
not affect the initial native E3D TTY baseline. The focused Rust tests above were
run from the branch worktree, which does not contain that inaccessible directory.
