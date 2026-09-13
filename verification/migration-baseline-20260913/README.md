# AMS 8000 verification (2026-09-13)

Machine-specific E3D 3.1 acceptance scripts. These are not deployment scripts.
Raw logs, copied databases, configuration, dependencies and binaries remain local.

## Latest verified run

- Source HEAD: `3ad3a15c3477ae0727ae0ecb24e9cf97b0fecc91` (`gen-model-refactor`).
- Evidence directory: `http-20260913-111410` (local, not committed).
- Build: `cargo build --bin aios-database --target-dir D:/work/plant-code/old/gen-model-refactor-target --message-format=json-render-diagnostics`, exit 0.
- Tested binary SHA-256: `e9a1b137f5e97c22d1e6f5c10b4e2bbab5b29b10d8fa3d58eee4ab8540f4d717`.
- Runner exit 0; `passed: true`; `room_checks: skipped`.
- Eight concurrent force-generation requests returned HTTP 200. This assertion alone does not count producer executions.
- Temporary BOX `24384/26467`: create, resize XLEN 1000 to 1500, delete using native SAVEWORK.
- Attribute and geometry responses changed after resize; deletion returned HTTP 404.
- Regenerated original root `24384/24776`: complete model items exactly matched the pre-create baseline.
- Native guarded mutation sessions: create 551 -> 553; resize 553 -> 555; delete 556 -> 558.
- Recovery was logical deletion of the temporary BOX, not bytewise database restoration. SAVEWORK advances sessions.

## Reproduction

Run from `D:/work/plant-code/old/gen-model-refactor` on the prepared AMS host:

```powershell
rtk proxy cargo build --bin l3_suite --target-dir D:/work/plant-code/old/gen-model-refactor-target
$env:AMS_NATIVE_CRUD = '1'
$env:AMS_SKIP_ROOMS = '1'
rtk proxy python verification/migration-baseline-20260913/run-room-http.py
```

`AMS_NATIVE_CRUD=1` opts into writing the AMS database. Without it, the runner only reads native inputs. `AMS_SKIP_ROOMS=1` disables room membership and room assertions.

Prerequisites: repository DbOption configured for AMS, E3D 3.1 test launcher/runtime, shared schema files and the existing Surreal executable. The runner builds its own backend; it only borrows the storage executable from the sibling directory. Native inputs and equipment fixture names are host-specific.

The native helper deletes only its uniquely named temporary BOX after a fresh identity probe. If native execution fails before the RefNo is captured, inspect its create log and clean up that exact temporary element before retrying. Retain the database backup for recovery investigation; never overwrite a live database from this backup.

## Remaining acceptance

OWNER moves and file replacement/recovery have not passed this run. Room tests were deliberately skipped. No deployment package was produced. The full library test results from earlier commits are not represented as a fresh full-suite run at this HEAD.
