# Lazy MDB watcher / HTTP repair verification

Date: 2026-09-12. Workspace: `D:/work/plant-code/old/gen-model-refactor`.

## Changes

- Removed the second MDB skeleton refresh from `sweep_watch_dirs` in both data faces. The read-through generation poller still owns targeted model refresh.
- Removed attribute/UDA provider initialization before HTTP bind. `start_configured` waits for a successful bind before the read-through route/room background work starts; a bind failure propagates to startup.
- Health reads the same route directory that the read-through watcher updates, plus the already-created HTTP providers' actual open sessions/cache counts. Missing declared files and unreadable generations are visible. Health performs no provider initialization or source reads.
- Removed all-member header reopening from the attribute freshness fingerprint. File length and modification time still invalidate a changed source; session reads remain in the generation poller and target opens.
- Updated API/source guards for the intentional health contract change. Updated existing SYS failure guard to permit project identity when a damaged SYS header cannot supply a dbnum. UI fallback guard checks the actual fallback expressions rather than the number of occurrences of `window.location.origin`.

## Commands and results

Commands run from the workspace with `rtk proxy`:

1. `cargo test --lib --test refno_native_guards --test refno_native_api_contract -- --test-threads=1`
   - `lazy-health-tests-verified.log`: library `1826 passed; 0 failed; 105 ignored`, exit status 101 because the following old API contract still required the deliberately removed `stale_reason`/`skeleton_elements` fields.
   - The remaining small source change after this library run removed header reads from `membership_fingerprint`; it was verified with the rebuilt live HTTP test below.
2. After updating that API contract and anchoring the source guard to actual function definitions: `cargo test --test refno_native_guards --test refno_native_api_contract -- --test-threads=1`
   - `lazy-health-contracts-final.log`: `4 passed; 0 failed` and `11 passed; 0 failed`; exit status 0.
3. `cargo build --bin aios-database`
   - `lazy-health-build-verified.log`: `Finished dev profile`; exit status 0.
4. `python verification/run-ams-lazy-http.py`
   - `ams-lazy-http-run.log`: `{"passed": true, "ready_ms": 6125}`; exit status 0.
   - Config, binary SHA-256, PID, ephemeral ports, full literal HTTP responses and shutdown result are in `ams-lazy-runtime/result.json`.
5. `python verification/package-lazy-health-evidence.py`
   - `lazy-health-rollback.log`: `rollback verified: 11 files`; exit status 0. Rollback executed against disposable copies of all modified files and verified original SHA-256 values; working changes remain installed.

## AMS observations

- Isolated spawned memory store, namespace 91516, `room_membership=false`, `startup_autorun=false`; separate working directory, copied runtime resources/config, original source database files read only.
- First health with a populated directory: 6125 ms, 70 routes, 0 cached elements, 0 open HTTP provider dbnums, 0 scans / 0 scanned elements. The five cumulative database opens were SYS membership reads.
- Eight concurrent first attribute requests for `24384/23257` returned identical nonempty responses from dbnum 8000. The only retained provider dbnum was 8000.
- Cumulative opens after the eight requests: 12, versus 572 in the earlier probe before fixing freshness checks. Both probes had zero element scans.
- A subsequent attribute read took 31 ms; cumulative opens stayed 12 and scans stayed zero. This verifies that a warm request no longer reopens all MDB file headers.
- `failed_dbnums=[7355]`: the configured project sources do not include the APS file declared by AMS. This is reported as degraded, without substituting a different project's same-number database.
- Probe server and its child store were stopped by the harness (`taskkill` exit 0). The server's exit status 1 reflects this deliberate forced shutdown, not a startup/request failure.

## Remaining acceptance gates

This is not full-plan acceptance or a release package. Attribute/tree providers still rebuild at MDB service granularity on a file identity change; sharing `DbSet`, strictly per-dbnum cache invalidation, independent DICT refresh, catalogue dependency invalidation, live SAVEWORK/restore geometry/owner parity, room-on acceptance, and JEU acceptance remain. The required memory store connection currently precedes HTTP startup; this change synchronizes bind before the optional route/room work, not before every startup operation. The ingest startup path also needs final bind-order coverage. Global NAME search still needs its explicit background-index gate.

No old 0.1.23 release package was overwritten. The focused patch is `lazy-health.patch`; its original/modified hashes are in `lazy-health-hashes.json`. Run rollback only against the unchanged repaired state with `python verification/lazy-health-rollback.py --root <workspace>`; it rejects later edits.
