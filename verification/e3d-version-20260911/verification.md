# E3D version selection verification

Working directory: D:\work\plant-code\old\gen-model

Baseline command:
`Select-String -LiteralPath verification/e3d-version-20260911/original/DbOption.toml,verification/e3d-version-20260911/original/src/options.rs,verification/e3d-version-20260911/original/src/data_interface/direct_store.rs -Pattern e3d_version`
Literal output: none (0 matches). PowerShell exit: 0.
Baseline behavior: shared loader only selected AIOS_E3D_TEMPLATE_DIR or its default; no TOML version field.

Modified command:
`rtk proxy cargo test --lib explicit_e3d_version -- --nocapture`
Exit: 0. Full literal output: tests.log.
Result: 4 passed; 0 failed; 0 ignored; 0 measured; 2129 filtered out.
Inputs: e3d210 and e3d31 packs; invalid IDs e3d99, empty string, auto; TOML with e3d210, missing key and array type; subprocess DB_OPTION_FILE pointing to a temporary TOML with e3d210, AIOS_E3D_TEMPLATE_DIR pointing to a missing directory, E3D210_SCHEMA_DIR unset.
Modified behavior: explicit TOML version takes the shared loader's early return and parses the selected pack without database detection. No version preserves the original directory path. Invalid selection errors.

`rtk proxy git apply --check --reverse verification/e3d-version-20260911/change.patch`
Exit: 0. Literal output: none (apart from RTK hook notice).
`rtk proxy git diff --check`
Exit: 0. Literal output: none (apart from RTK hook notice).

Rollback command (executed on disposable copies, current changes retained):
`& verification/e3d-version-20260911/rollback.ps1 -TargetRoot D:\work\plant-code\old\gen-model\verification\e3d-version-20260911\rollback-check`
Exit: 0.
Literal output: ROLLBACK_OK: 3 original files restored and SHA256 verified

Original and modified file SHA256 records: baseline-hashes.json, modified-hashes.json.
Modified artifacts: DbOption.toml, src/options.rs, src/data_interface/direct_store.rs in the working directory.
No production service restart or deployment was performed. Tests exercised the compiled library and shared loader in a child process.
