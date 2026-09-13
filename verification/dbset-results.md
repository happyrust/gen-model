# DbSet generation-sharing foundation

2026-09-12. Production changes are in `D:/work/plant-code/old/vendor/e3d-io/src/db_element.rs`; the consumer checkout remains `D:/work/plant-code/old/gen-model-refactor`.

## Changed behavior

- Concurrent first reads of the same dbnum serialize resolver/open work under a per-dbnum lock. Different dbnums do not hold the same opening lock. Successful concurrent first reads now resolve/open once, rather than opening multiple engines and discarding the losing ones.
- Parsed/extracted element caches are grouped by dbnum. Invalidating a dbnum removes that cache bucket instead of walking every cached element in the MDB.
- `DbSet::fork_without(changed, resolver)` creates a new view, sharing unaffected open engines and cache buckets by Arc. Changed dbnums are left unopened in the new view. Existing readers can retain the old DbSet and its frozen data. Forking performs no file reads or per-element cache copies.
- Pool opens validate the actual file database type against the requested type in addition to existing database-number validation and template validation.
- `cached_element_count` exposes the retained parsed-element count for later shared-service health reporting.

The new view API assumes published views are refreshed by forking. This batch does not redefine concurrent in-place `add_db` replacement as a multi-operation snapshot transaction.

## Verification

Exact shell commands were prefixed with `rtk proxy`.

- In `vendor/e3d-io`: `cargo check` — exit 0, `dbset-check-final.log`.
- In `vendor/e3d-io`: `cargo test --lib` — exit 0; **249 passed, 0 failed, 2 ignored**; `dbset-lib-tests.log`.
- In `vendor/e3d-io`: `cargo test --test dbset_generations --test db_element_facade -- --include-ignored --test-threads=1 --nocapture` — exit 0; **6 existing facade tests and 2 generation tests passed**; `dbset-tests-final.log`.
  - Synthetic files prove handle creation does no I/O; eight simultaneous first reads resolve and open once; generation 1→2→1 changes only the selected dbnum; old views stay readable; another dbnum retains the same parsed Arc; number/type mismatches fail. Zero scans.
  - AMS reads branch `24384/23257` from dbnum 8000 and the descriptor's world reference from CATA 5052. The next view invalidates only 8000. CATA extraction retains the identical Arc, the prior branch extraction remains readable, and reading the new branch adds exactly one database open. Literal output: `AMS fork: catalogue Arc retained, old branch Arc retained, design opens delta=1`; total 3 opens, 0 scans, 3 point lookups.
  - Existing facade tests may explicitly scan for reference truth; their scans are not startup/request scan measurements.
- In `gen-model-refactor`: `cargo check --lib` — exit 0, `dbset-consumer-check.log`.
- In `gen-model-refactor`: `cargo test --lib -- --test-threads=1` — exit 0; **1828 passed, 0 failed, 106 ignored**; `dbset-gen-model-regression.log`.
- `python verification/package-dbset-evidence.py` — exit 0, `rollback verified: 2 files`. The rollback was executed against disposable copies and original bytes verified; repaired source remains installed.

## Artifacts and remaining work

- Original source: `dbset-original/db_element.rs`.
- Focused diff: `dbset.patch`; original/modified SHA-256: `dbset-hashes.json`.
- Runnable rollback: `python verification/dbset-rollback.py --root D:/work/plant-code/old/vendor/e3d-io`. It validates every current hash before restoring the source and removing the newly added test. Later edits cause rejection.

This is a tested prerequisite, not completed HTTP service integration. Attribute and tree services still need a shared current-view owner using this fork API, including target file identity checks, per-dbnum view publication, and accurate shared cache statistics. Tree metadata must become lazy so a single ancestor/root lookup does not initialize all DESI metadata. Full SAVEWORK/restore, catalogue dependencies, room-on, JEU, and packaging gates remain outstanding. No release package was replaced.
