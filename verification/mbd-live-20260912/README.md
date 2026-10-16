# MBD live HTTP baseline — 2026-09-12

This directory records task `t-440`: diagnose the existing `:8022` failure,
leave the user's process untouched, and prove the main `gen-model` repository
against real AvevaMarineSample files on an isolated port.

## Safety boundary

- The process already listening on `127.0.0.1:8022` was inspected only. It was
  not stopped, restarted, or reconfigured.
- The evidence run used `127.0.0.1:18084`, `store_mode=embedded-mem`, a unique
  temporary working directory, `startup_autorun=false`, and the startup room
  build stop switch.
- The runner stopped only the child process that it created. A final port
  check found PID `79984` still listening on `8022` and no listener on `18084`.
- No business source file or global environment setting was changed.

## Existing `:8022` diagnosis

The listener is not the current main-repository executable:

- PID: `79984`
- executable:
  `C:\Users\dpc\AppData\Local\Temp\gen-model-p5-spawned-mem-e3fde0d72244456ca2062ec893f9e9b6\aios-database.exe`
- command: `aios-database.exe serve`
- executable SHA-256:
  `8D94C169E52FC7ACF52632AD5BFBB65E3049914C9E9AFA6950788381E0C2EED1`
- reported build:
  `0.1.22+g757b23e94390.1789097302.dirty`
- storage: `spawned-mem` at `127.0.0.1:18809`
- health: HTTP 200, service `degraded`, core `stale`
- MBD request: HTTP 500

The 500 is a stale runtime configuration, not an MBD solver failure. The
running process points at the now-missing testbed tree:

```text
D:\work\plant-code\old\gen-model\python\testbed\projects\AvevaMarineSample\ams000\ams1112_0001
```

The exact response says `os error 3` (`系统找不到指定的路径`). See
`current-8022-24383_99996.http.txt`; the complete health body is
`current-8022-health.json`.

Replacing PID `79984` automatically would destroy an existing user process, so
this run established a separate live baseline instead.

## Isolated main-repository baseline

The main repository was built with:

```powershell
cargo build --bin aios-database
```

Identity captured by the runner:

- repository: `D:\work\plant-code\old\gen-model`
- branch: `e3d-direct`
- revision: `cf168862aadf9d2744dd2be9dc20a8c2ff270ed3`
- worktree: dirty; the binary hash is therefore the authoritative build
  identity
- executable: `D:\Rust\target\debug\aios-database.exe`
- executable SHA-256:
  `C2A41AB6988E75BEE0DFAF5CBD65C316E122C963C2EE1F98CDC7BA03021EF46E`
- crate version: `0.1.22`

The isolated health endpoint returned HTTP 200 and top-level status `ok` with:

- project `AvevaMarineSample`, MDB `/ALL`, namespace `1516`
- `data_read_mode=direct`, `data_face=read-through`
- real-file session vector entries:
  - dbnum `7997`, sesno `180`
  - dbnum `7999`, sesno `194`
  - dbnum `8000`, sesno `444`

`isolated-18084-health.json` is the unmodified response body. The temporary
configuration used by the run is `isolated-18084-config.toml`.

## HTTP results

All expected statuses matched.

- `24383_99996` — HTTP 200, valid V2, `isodim_main`, source mm plus a
  16-element transform; 58 primitives:
  8 `linear_dim`, 11 `aid_line`, 5 `aid_text`, 17 `label`,
  17 `leader_line`; 13 non-error issues; 12 isolines, 6 empty.
- `24381_145018` — HTTP 200, valid V2, `isodim_main`; 77 primitives:
  18 `linear_dim`, 7 `slope_mark`, 21 `aid_line`, 3 `aid_text`,
  14 `label`, 14 `leader_line`; zero issues; 11 isolines, 1 empty.
- `24384_26229` — HTTP 200, valid V2, `isodim_main`; 39 primitives:
  5 `linear_dim`, 1 `slope_mark`, 1 `weld_mark`, 6 `aid_line`,
  2 `aid_text`, 12 `label`, 12 `leader_line`; 5 non-error issues;
  6 isolines, 3 empty.
- malformed `not-a-refno` — HTTP 400, code `bad_request`.
- missing `24383_999999999` — HTTP 404, code `not_found`.
- existing non-route FTUB `24383_99997` — HTTP 422, code `precondition`.

The three successful raw files include HTTP headers and the complete JSON
body. `summary.json` contains machine-readable counts, statuses, build
identity, sessions, and links to every raw response.

## Typed contract check

The existing real-AMS HTTP integration test was rerun:

```powershell
cargo test --test mbd_http_endpoint -- --nocapture
```

Result: `1 passed; 0 failed`. That test deserializes a successful HTTP body as
`plant_mbd::MbdV2PipeData` and also covers 400/404/422. Raw output is in
`typed-parse-test.log`.

## Reproduce

From the main repository:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass `
  -File .\verification\mbd-live-20260912\run.ps1
```

The script refuses an occupied isolated port, never touches the `:8022`
process, writes complete responses into this directory, stops its own child,
removes its temporary workspace, and refreshes `summary.json` and
`SHA256SUMS.txt`.

## Held service handoff (`t-461`)

A second isolated instance was held at `http://127.0.0.1:18084` for the
frontend `t-441` browser acceptance:

- PID `72048`
- the same isolated configuration and main-repository debug executable
- executable SHA-256
  `C2A41AB6988E75BEE0DFAF5CBD65C316E122C963C2EE1F98CDC7BA03021EF46E`
- readiness: health 200 / `ok` / core `ready`; all three real BRAN endpoints
  returned 200

The frontend acceptance reported `24381_145018` loaded through the live
service into the ordinary `:3101` page:

- MBD response: 200 / 77 primitives
- loaded model: dbnum 7997, 12 refnos, 22 DTX objects
- 77 `source=mbd` layouts
- source records stayed stable across camera rotation and 6x zoom-out
- 86 arrow wings held the 13 px readability floor
- editable `DimensionDocument` remained empty
- SVG and two screenshots were saved by the frontend task
- all of its structured checks were true

After that explicit completion notice, only PID `72048` was stopped.
`held-cleanup.json` proves:

- pre-stop health was 200
- PID `72048` and its console child are absent
- port `18084` is free
- the unique temporary run directory is absent
- the isolated store was in-process `embedded:mem`, so it had no external
  store port to leave behind
- existing PID `79984` still listens on `8022`

Startup identity and readiness are in `held-state.json` and
`held-ready-health.json`; complete process output is in `held-stdout.log` and
`held-stderr.log`.

## Non-blocking warnings

The isolated log records a missing foreign SCB design member
`6003 (*MASTER/SCBDESI)` and an empty spatial-tree rebuild in the temporary
workspace. They did not affect the three MBD endpoints: health was `ok`, all
three returned typed V2 data, and every requested status matched.
