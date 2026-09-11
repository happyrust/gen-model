# Manual E3D schema selection after database-field audit

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
- `DB_Communication::clearComm()` clears `COMM.VERS` on logout/expunge.
- The E3D 3.1 AMS Communication database has current indexed values from
  different clients/modules (`3.1.9.0` and `1.3.13.0`), while idle projects
  commonly have no value.

The full IDA addresses and hashes are recorded in
`../../../vendor/e3d-io/docs/evidence/2026-09-11-e3d-database-version.md`.

## Implemented policy

- `DbOption.toml.e3d_version` is required and is the only SchemaPack selector.
- The manual value opens that pack immediately; no database probe or
  automatic mismatch block runs.
- A missing value fails with a targeted configuration error even if
  `AIOS_E3D_TEMPLATE_DIR` or recognizable-looking `COMM.VERS` text exists.
- e3d-io keeps `COMM.VERS` and `UPGDNO` in its diagnostic output, but
  `detect_*` / `SchemaPack::detect_*` fail loudly instead of assigning a
  release.
- Directory names, timestamps, file sizes, `WORL.UPGDNO`,
  `Saving Meta Data.`, and `COMM.VERS` are not selection inputs.

## Directed verification

- e3d-io schema tests: 14 passed, 2 live tests ignored in the normal run;
  they cover fail-loud automatic entry points, diagnostic extraction,
  truncated sessions, shared `UPGDNO`, and ignored path names.
- live E3D 3.1 diagnostics retain both AMS client/module versions but assign
  no release.
- copying 3.1 `amscom` under a directory named `E3D2.1` preserves diagnostics
  and still assigns no release.
- gen-model explicit-version tests: 4 passed.
- gen-model missing-version test proves that neither a real project path nor
  `AIOS_E3D_TEMPLATE_DIR` re-enables guessing; passed.
- additional project inspection established why `COMM.VERS` is unstable:
  AvevaCatalogue is an identical untouched copy; SCB and ZDJ have current
  empty values after communication cleanup.

## Full checks

- e3d-io `cargo test --lib`: 242 passed, 2 ignored.
- e3d-io `cargo check --all-targets`: passed; two pre-existing example
  dead-code warnings.
- e3d-io Clippy: no warning in the changed version-selection files; the
  existing library reports 24 unrelated warnings.
- gen-model `cargo check --lib`: passed.
- gen-model Clippy: passed for this crate; one pre-existing dead-code warning
  comes from `e3d-model`.
- gen-model `cargo test --lib`: the five version-selection/configuration tests
  passed. The repository-wide result was 1989 passed, 136 ignored, and 9
  failures in unrelated pre-existing model-finalization, pe-batch,
  window-root-plan, and embedded-web-bundle tests; none of those files is part
  of this change.
- rustfmt checks, `git diff --check`, and IDE diagnostics passed for the
  changed Rust files.

No production service was restarted and no deployment was performed.
