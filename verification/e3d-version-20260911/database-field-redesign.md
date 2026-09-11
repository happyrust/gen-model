# Database-field E3D schema selection

## Corrected evidence

The original plan assumed that the two `amssys` files contained different
release values. Direct record reads and IDA disprove that assumption:

- E3D 2.1 and E3D 3.1 `amssys` both store `WORL.UPGDNO=12010401`.
- E3D 2.1 and E3D 3.1 `ams8000_0001` both store
  `WORL.UPGDNO=12010104`.
- `DB_DB::upgdno()` reads a database migration number; it is not the E3D
  product release.
- `DB_Communication::updateVersion()` writes `VersionInfo::version()` into
  `COMM.VERS`.
- The E3D 3.1 AMS Communication database has current indexed
  `COMM.VERS=3.1.9.0`.
- The E3D 2.1 AMS Communication database has no measured product-version
  value, so strict automatic selection returns `Unknown`.

The full IDA addresses and hashes are recorded in
`../../../vendor/e3d-io/docs/evidence/2026-09-11-e3d-database-version.md`.

## Implemented policy

- `DbOption.toml` with `e3d_version` is manual mode. It opens that SchemaPack
  immediately and does not inspect project databases or block on a mismatch.
- Without `e3d_version`, gen-model asks e3d-io for current project-level
  `COMM.VERS` evidence.
- Discovery is limited to `included_projects` (including `project_dirs`
  remapping); an excluded project cannot lend its version to the configured
  scope. Multiple included projects use the highest measured version.
- A measured `3.1.*` value selects `e3d31`; measured `2.1.*` / `12.1.*`
  values select `e3d210`.
- Missing or unfamiliar evidence stops with an explicit error.
- Directory names, timestamps, file sizes, `WORL.UPGDNO`,
  `Saving Meta Data.`, and `AIOS_E3D_TEMPLATE_DIR` are not automatic
  selection inputs.

## Directed verification

- e3d-io schema unit tests: 15 passed, 2 live tests ignored in the normal run.
- live E3D 3.1 project evidence: passed (`COMM.VERS=3.1.9.0`).
- copied 3.1 `amscom` under a directory named `E3D2.1`: passed as `e3d31`.
- E3D 2.1 AMS with no measured `COMM.VERS`: returned `Unknown`.
- gen-model explicit-version tests: 4 passed.
- gen-model missing-evidence test: passed.
- gen-model live automatic selection: passed and opened
  `schema/e3d31/binaries`.
- additional E3D 3.1 projects: AvevaPlantSample, TEST, YCYK-E3D and the SLYK
  YCYK copy selected `e3d31`; AvevaCatalogue, SCB and ZDJ returned `Unknown`
  because their current COMM records carry no measured product version.

## Full checks

- e3d-io `cargo test --lib`: 243 passed, 2 ignored.
- e3d-io `cargo check --all-targets`: passed; two pre-existing example
  dead-code warnings.
- e3d-io Clippy: no warning in the changed version-selection files; the
  existing library reports 24 unrelated warnings.
- gen-model `cargo check --lib`: passed.
- gen-model Clippy: passed for this crate; one pre-existing dead-code warning
  comes from `e3d-model`.
- gen-model `cargo test --lib`: the six version-selection/configuration tests
  passed. The repository-wide result was 1989 passed, 137 ignored, and 9
  failures in unrelated pre-existing model-finalization, pe-batch,
  window-root-plan, and embedded-web-bundle tests; none of those files is part
  of this change.
- rustfmt checks, `git diff --check`, and IDE diagnostics passed for the
  changed Rust files.

No production service was restarted and no deployment was performed.
