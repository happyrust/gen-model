# Independent UDA catalog verification

2026-09-12 — `D:/work/plant-code/old/gen-model-refactor`.

The prior goal turn made verified startup/HTTP progress. This batch removes UDA ownership from `MdbMemoryStore` and removes its remaining `ensure` call from `spool_write_through`: a model write no longer builds an element skeleton just to obtain dictionary definitions.

`UdaCatalogProvider` shares a cache keyed by the ordered DICT paths and schema path. Its mutex serializes initial loading and reload publication. DESI changes do not invalidate this cache. Each attribute/ensure/spool operation obtains one immutable catalog; failed loading or changed file identities during loading reject the new snapshot and preserve the old Arc for existing readers. Restoring a DICT permits a later fresh load.

The HTTP attribute service excludes DICT from its whole-service membership fingerprint. It separately checks identities of already-open DirectStore sessions, including direct reads of DICT elements and dependency files. Thus ordinary DICT definition changes no longer discard unrelated DESI sessions. Whole-service rebuild on a changed DESI or an explicitly opened changed DICT session remains pending the per-dbnum store work.

## Validation

- `cargo check --lib`: exit 0 (`uda-provider-check.log`).
- `cargo test --lib uda_catalog_provider -- --test-threads=1`: 2 passed, 1 opt-in AMS test ignored; exit 0 (`uda-provider-tests-final.log`). Tests cover concurrent first reads, DESI isolation, DICT generation changes, restore, missing/corrupt input, and changes during loading.
- `python verification/run-uda-test-binary.py`: exit 0. Reuses the compiled test binary; exact binary hash, commands and statuses are recorded in `uda-test-commands.json`.
  - Complete default library suite: **1828 passed, 0 failed, 106 ignored** (`uda-provider-lib-regression.log`). The subsequently formatted provider has the same logic.
  - Explicit AMS DICT copy test: **1 passed** (`uda-provider-ams-copy.log`). Six DICT files, 493 definitions, corrupt-copy failure, restore success, prior snapshot preserved, 12 database opens, **0 scans / 0 scanned elements**. Source database files were not modified.
- `cargo test --test refno_native_guards --test refno_native_api_contract -- --test-threads=1`: **12 + 4 passed**, exit 0 (`uda-provider-contracts.log`). New guards prohibit UDA ownership in the skeleton and any skeleton dependency in spool rendering.
- `cargo build --bin aios-database`: exit 0 (`uda-provider-build.log`).
- `AMS_PROBE_OUTPUT=<workspace>/verification/ams-uda-runtime python verification/run-ams-lazy-http.py`: exit 0, **5531 ms** to populated-route health; 8 concurrent first attribute requests and a repeated read passed. Exact HTTP bodies, binary hash, ports, config and shutdown statuses are in `ams-uda-runtime/result.json`. Test store and HTTP were isolated and stopped afterward.
- `python verification/package-uda-evidence.py`: validates the patch/hashes and executes rollback against disposable copies; see `uda-provider-rollback.log`.

Original files: `uda-provider-original/`. Current modified artifacts: five existing files plus the new `src/data_interface/uda_catalog_provider.rs`, all listed in `uda-provider-hashes.json`. Focused patch: `uda-provider.patch`. Runnable rollback: `python verification/uda-provider-rollback.py --root <workspace>`; later edits are rejected before any restoration.

This is not full-plan acceptance. Shared DbSet, strictly per-dbnum store/cache invalidation, catalogue dependency invalidation, live SAVEWORK/restore model/owner/geometry parity, room-on tests, JEU acceptance and release packaging remain active work. No existing 0.1.23 package was overwritten.
