# Review fixes and AMS verification

Scope: gen-model-refactor current dirty worktree; preceding user changes preserved in review-fix-original, SHA-256 manifest in review-fix-hashes.json.

Changes: keep invalidated roots discoverable independently of valid receipts; remove root inventory only after successful retirement; distinguish declared-but-unavailable files from outside-MDB references; read BASESN and intervening session chain during changed-file polling.

Verification:
- cargo test --lib invalidating_one_dbnum -- --nocapture: exit 0, 1 passed.
- cargo test --lib mdb_route_directory -- --nocapture: exit 0, 15 passed, 1 ignored (before additional BACKTRACK test).
- cargo test --lib -- --test-threads=4: exit 101; 1823 passed, 2 failed, 104 ignored. Includes new skipped-BACKTRACK and missing-file tests passing. Full literal output: review-fix-lib.log.
- cargo test --lib live_directory_matches_every_file_header -- --ignored --nocapture: exit 0, 1 passed, AMS 70 routes/29 element databases. Literal output: review-fix-ams.log.
- Contract rerun output: review-fix-contracts.log.
- Rollback tested against temporary copies: rollback verified. Production worktree not rolled back.

Remaining failures: increment_manager::watermark_or_data_backing_read_failure_becomes_a_phase_blocker; handlers::plant_ui_web_bundle_contains_bootstrap_and_runtime_assets. These files were not changed by this fix; no pre-fix full-suite run was captured, so origin is unproven.

Limits: no live SAVEWORK mutation or restart/crash replay acceptance performed. Tree L5 still pending. Retained root inventory is process-local and is not a durable retry queue; failed refresh remains visible for subsequent SAVEWORK or explicit ensure, without automatic generation retries.
