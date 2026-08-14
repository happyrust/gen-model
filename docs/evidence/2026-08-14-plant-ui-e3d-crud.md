# Plant UI + E3D incremental CRUD verification

Date: 2026-08-14
Code under test: 48e3f4bb (strict initialization implementation c4865ea5 plus evidence commits)

## Equipment fixture (db7999)

- Preflight: PASS; DESI dbnum=7999, WORLD=16191/0, 9 scenarios.
- Plant UI runtime initially failed because target/debug lacked resource/surreal; after copying the test-workspace runtime assets, UI connected with 0 ERROR.
- UI locator defect: fixture objects resolve by name/refno but the command remains at "expanding ancestor path"; 8 UI smoke cases could not become visible in the model tree. Evidence: D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\plant-ui-crud-20260814-180107\equipment-run-fixed-181855\summary.json and D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\plant-ui-crud-20260814-180107\equipment-run-fixed-181855\data\ui-before.png.
- Backend four-plane results: PASS data, transform, room-structure, room-member; FAIL geometry, boolean, owner, add, delete because the expected generation root is absent from the task receipt. Evidence: D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\plant-ui-crud-20260814-180107\equipment-data-182722\summary.json.
- Every applied scenario ran its paired restore; suite teardown completed.

## Pipeline model (db8000 + Plant UI)

Baseline: file_latest=222, applied=222. Plant UI located BRAN /C-IY-1R330-A and displayed its tree/attributes with data source ready.

FTUB owner move:
- Session 223: FTUB 24384/22403 moved owner 24384/22402 -> 24384/22404 (3 Modified operations).
- Session 224: restore moved it back to 24384/22402.
- Batch db-20260814-183950-000000 succeeded for 223..=224, events_seen=2, changed_elements=6.
- Final file_latest=224, applied=224; Surreal owner=pe:24384_22402, sesno=224.
- Plant UI after refresh: queue 0, ERROR 0, "loaded elements 120 -> 120"; before/after screenshots retained.
- Defect reproduced: task result merged_sesnos=[] and merged_sesno_times=[] for the two-session frozen window.

GENSEC add/delete:
- Three direct/check-driver attempts crashed in the E3D runtime before SAVEWORK; file remained at 224 and /CODEX_L3_GENSEC is absent.
- No delete macro was needed because no add session landed.

## Verified artifact roles

- Modified artifact: D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001 (session history 223/224; final business state restored).
- Patch/diff: D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\plant-ui-crud-20260814-180107\pipeline-ui-183559\semantic-window-223-224.json.
- Verification record: D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\plant-ui-crud-20260814-180107\pipeline-ui-183559\task-ftub.json, D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\plant-ui-crud-20260814-180107\pipeline-ui-183559\db8000.after-ftub.json, D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\plant-ui-crud-20260814-180107\pipeline-ui-183559\surreal.after-applied.txt, and this file.
- Rollback: D:\work\plant-code\old\test-worklspace\bin\.codex-deploy\plant-ui-crud-20260814-180107\pipeline-ui-183559\rollback.ps1 (executed exit 0; port 9099 closed; project mirror removed).

Pre-existing E3D PIDs 35528, 58016, 68872 were preserved. SurrealDB 8009 remained running. No repository source/config was replaced.
