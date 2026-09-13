# Shared MDB read view — live AMS HTTP acceptance (attributes → ancestors → children)

Date: 2026-09-12 18:54 (+08:00). Workspace: `D:/work/plant-code/old/gen-model-refactor`, branch
`refactor/retire-publish-stage`, HEAD `207d5c5d1e9772793937040ca11d5fc9681f67d2` with uncommitted
working changes (`build_id = 0.1.23+g207d5c5d1e97.1789210437.dirty`).

Task: prove on the binary built from the current working tree, against an isolated AMS HTTP
instance, that reads perform zero element scans, never reopen an already open database, share one
open-database set between the attribute and tree services, and answer concurrent requests
byte-for-byte like serial ones. No production code was changed; `run-ams-lazy-http.py` was not
touched; no git commit; source database files were only read.

## Binary and build

| Item | Value |
|---|---|
| Build command | `cargo build --bin aios-database` (cwd = workspace, `CARGO_TARGET_DIR=D:\Rust\target`) |
| Build exit status | `0` (`Finished dev profile … in 1m 12s`; the log starts with `Blocking waiting for file lock on build directory` — another cargo held the shared target dir, waited, not killed) |
| Binary | `D:/Rust/target/debug/aios-database.exe`, 216,001,536 bytes, written 2026-09-12T18:54:10+0800 |
| Binary SHA-256 | `30412177ee120906b7ec019d2512cdc6b8300715f756602bfe6aeb09389da87b` |
| Build log | `verification/ams-shared-read-runtime/build.log` |

Timeline note: an earlier attempt at 18:47 failed with `E0716` in
`vendor/e3d-io/src/record/text.rs:126` — another session was mid-edit in `vendor/e3d-io`
(`record/text.rs`, `engine.rs`, `db_element.rs`, … all rewritten 18:45–18:51). That is outside this
task; nothing in `e3d-io` was touched by it. The tree compiled again at 18:51 and the run below is on
the binary built at 18:54 from the tree as it then stood (including those in-flight `e3d-io` changes).

## Isolation

Script: `verification/run-ams-shared-read-http.py` (new; same isolation as `run-ams-lazy-http.py`).

- Temporary `DbOption.toml` copied from the workspace with `surreal_ns = 91517`, `v_port = 64967`,
  `http_api_addr = "127.0.0.1:64966"`, `server_release_ip = "127.0.0.1:64966"`,
  `startup_autorun = false`, `room_membership = false`; ephemeral ports checked free before start.
- `DB_OPTION_FILE` → the temporary file; `AIOS_SURREAL_EXE` = `gen-model/bin/surreal.exe`
  (spawned memory store, `sul_db.endpoint = localhost:64967`); `AIOS_E3D_SCHEMA_ROOT` =
  `vendor/e3d-io/schema`; `AIOS_OPEN_BROWSER=0`; `PLANT_ASSET_ROOT` inside the runtime dir.
- cwd = `verification/ams-shared-read-runtime/` (instance lock, logs, `accel_tree_*.snapshot` land
  there, not in the workspace or the project directory).
- Server PID 22988, `aios-database serve`, `data_face = read-through`, project `AvevaMarineSample`,
  MDB `/ALL`, namespace `91517`. HTTP ready after 6,141 ms.
- Shutdown by the script: `taskkill /PID 22988 /T /F` → exit 0, three processes terminated
  (server + two children, one of them the spawned store). Server `process_exit_status = 1` is the
  forced kill, not a request failure.

Source files: every file below `D:/AVEVA/Projects/E3D3.1` was SHA-256'd before the build and after
shutdown — 13,065 files, 14,909,438,101 bytes, 0 unreadable, both passes. Manifest SHA-256
`d8574f1104d64813a9c2b649372cdb6adf5b6238f99db31af0562cc8930abd0a` before and after; `source_changed_files = []`.
Per-file digests: `ams-shared-read-runtime/source-hashes-before.json` / `source-hashes-after.json`.

## Health counters per step (verbatim from `result.json`)

`core.route_count` stayed `70` throughout; `scan_calls` and `scanned_elements` stayed `0` throughout.
Health itself did not move any counter (the 30 s settle loop after cold start saw three identical
readings in 1,140 ms; `dabacon_tags.status = pending, refreshing = false` at start and end).

| Step | Request | Elapsed | `open_db_count` / `open_dbnums` after | `source_reads` after | Δ `database_opens` | Verdict |
|---|---|---|---|---|---|---|
| (a) cold | first `GET /api/v1/health` with routes | — | `0` / `[]` | `{"database_opens": 5, "scan_calls": 0, "scanned_elements": 0, "index_point_lookups": 409}` | — | **PASS** `open_db_count == 0`, `scan_calls == 0`, `route_count == 70`. The 5 opens are the SYS membership reads at startup (same as `lazy-health-results.md`). |
| baseline | health after settle | — | `0` / `[]` | `{"database_opens": 5, "scan_calls": 0, "scanned_elements": 0, "index_point_lookups": 409}` | 0 | unchanged since cold |
| (b1) attributes | `POST /api/v1/element/attributes` `{"refno": "24384/23257"}` | 328 ms | `1` / `[8000]` | `{"database_opens": 12, "scan_calls": 0, "scanned_elements": 0, "index_point_lookups": 1612}` | **+7** | **PASS** — response `dbnum = 8000`, `noun = BRAN`, 92 attributes; `+7 = 1 + 6`: one element database (8000, the only dbnum retained by the shared read view) plus **6 dependency opens**. |
| (b2) ancestors | `GET /api/v1/tree/ancestors?refno=24384/23257` | 15 ms | `1` / `[8000]` | `{"database_opens": 12, "scan_calls": 0, "scanned_elements": 0, "index_point_lookups": 1616}` | **0** | **PASS** — chain `24384_23257 → 24384_23225 → 24384_22400 → 24384_22399 → 16192_0` (WORL = `RefNo(primary_extent_ref, 0)` of 8000); no new open, `open_db_count` unchanged. |
| (b3) children | `GET /api/v1/tree/children?refno=24384/23257` | 16 ms | `1` / `[8000]` | `{"database_opens": 12, "scan_calls": 0, "scanned_elements": 0, "index_point_lookups": 1625}` | **0** | **PASS** — 9 members (FTUB/BEND), every node `dbnum = 8000`; no new open, `open_db_count` unchanged. |
| (c) 8 concurrent | 3 × attributes, 3 × ancestors, 2 × children in one `ThreadPoolExecutor(8)` burst | 0–31 ms per lane | `1` / `[8000]` | `{"database_opens": 12, "scan_calls": 0, "scanned_elements": 0, "index_point_lookups": 1625}` | **0** | **PASS** — all 8 lanes byte-identical to the serial responses (SHA-256 `be28fa60…` attributes 14,998 B, `f6a5aaa7…` ancestors 96 B, `eaaa0877…` children 1,681 B); `database_opens` stayed 12. |
| (d) roots | `GET /api/v1/tree/roots` | 31 ms | `29` / `[1112, 6003, 7009, 7011, 7015, 7022, 7027, 7048, 7321, 7324, 7326, 7327, 7329, 7330, 7331, 7332, 7333, 7334, 7350, 7352, 7353, 7354, 7356, 7741, 7997, 7998, 7999, 8000, 250705]` | `{"database_opens": 40, "scan_calls": 0, "scanned_elements": 0, "index_point_lookups": 1721}` | **+28** | **PASS** — 38 SITE nodes over 20 dbnums; exactly the 28 element databases not yet open were opened (8000 was reused), giving 29 = every element library with a file (`dbnum 路由目录 70 条路由（元素库 29 …）` in `stdout.log`). `scan_calls` still 0. |
| extra: roots again | `GET /api/v1/tree/roots` | 16 ms | `29` / same list | `{"database_opens": 40, "scan_calls": 0, "scanned_elements": 0, "index_point_lookups": 1721}` | 0 | informational — byte-identical to (d), nothing reopened. |
| extra: idle 2 s | health | — | `29` / same list | `{"database_opens": 40, "scan_calls": 0, "scanned_elements": 0, "index_point_lookups": 1721}` | 0 | informational — no background opens while idle. |
| (e) sources | SHA-256 sweep after shutdown | 24.7 s | — | — | — | **PASS** — manifest and all 13,065 per-file digests identical to the pre-run sweep. |

All 24 scripted checks passed (`result.json` → `checks[]`, `passed = true`); `failed_checks = []`.

### On the attribute dependency count

The recorded number is **6** (`attributes_dependency_opens = 6`; `+7` total). The shared read view
retained only dbnum 8000 (`open_dbnums = [8000]`, `newly_open_dbnums = [8000]`), so the six extra
opens were not held by the `DbSet`. They are consistent with the first `read()` building the UDA
catalogue (`element_attributes.rs` → `self.catalog.get()` → `UdaCatalogProvider::shared`) over the six
DICT databases the MDB declares (`stdout.log`: `其中字典库 6 个：盘上 6 个` — scb6002, ams5100, acp7043,
acp7006, acp7323, ams5101). The counter itself does not name the files; this attribution is by
count and code path, not by a per-file trace. The number was identical (12 after attributes) in
`lazy-health-results.md` and in a preliminary run of this script against the 18:41 binary
(`551fadab3866b9a5b62163ba3acec82c273a7d465de70c9ffee310509e2ecbda`, before the `e3d-io` edit landed;
artifacts kept outside the repository).

## Files

- `verification/run-ams-shared-read-http.py` — the probe (build → hash sources → serve → (a)–(d) →
  stop → hash sources → `result.json`). `AMS_PROBE_OUTPUT` relocates the runtime dir;
  `AMS_PROBE_BUILD=0` skips the build and records that it did so.
- `verification/ams-shared-read-runtime/result.json` — binary SHA-256, build command/exit status,
  git identity, ports, every response verbatim (`response_text`) with its SHA-256, health counters
  before/after every step, all checks, source manifests.
- `verification/ams-shared-read-runtime/{stdout,stderr,build}.log`, `source-hashes-{before,after}.json`.
- `verification/ams-shared-read-http-run.log` — the script's one-line summary
  (`{"passed": true, "ready_ms": 6141, "sha256": "30412177…", "build_exit_status": 0, "failed_checks": []}`).

`stderr.log` carries only the two pre-existing notices: dbnum 7355 (`*MDU/CATA`, declared by `/ALL`,
no file in the configured projects → `failed_dbnums = [7355]`, `core.status = degraded`) and the
spatial-tree snapshot rebuild in the fresh runtime dir. Neither is introduced by this change.

## Not covered

Single run, one refno, read-through data face, `room_membership = false`, `startup_autorun = false`.
Not exercised here: SAVEWORK/file-change invalidation of the shared view (`MdbReadProvider::view`
fork path), global NAME search, JEU, ingest data face, and load beyond 8 concurrent lanes.
