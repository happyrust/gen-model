# Shared read view: service wiring close-out, guard update, sharing proof

2026-09-12 — `D:/work/plant-code/old/gen-model-refactor`, branch `refactor/retire-publish-stage` (uncommitted working tree; nothing committed by this batch). Continues the shared-read batch whose pre-wiring originals sit in `shared-read-original/`.

Before this batch `DirectTreeService` and `ElementAttributeService` were already wired to `MdbReadProvider::shared → reader.view()`, but nothing proved they share one view, the source guards still described the previous shape (three guards red), and the tree service still imported the element skeleton's `LoadedDb`. Compilation passing (`cargo check --lib`) was the only evidence.

## Changed behavior

- `src/data_interface/direct_tree.rs`: the `mdb_memory_store::LoadedDb` import is gone. `TreeMetadata.databases` is now a local `TreeDatabase { dbnum, db_type, file, sesno }` (identity + pinned session read through the shared view; no `elements` count, there is no census). `session_fingerprint` / `epoch` / `databases()` keep their shapes. A `#[cfg(test)]`-only `read_view()` seam lives inside the existing `lazy_live_tests` module (not on the production `impl`, because the guards cut production code at the first `#[cfg(test)]` marker).
- `src/data_interface/element_attributes.rs`: the `open` doc comment no longer names `mdb_memory_store` and describes the shared read view. New opt-in AMS test `ams_tree_and_attribute_services_share_one_read_view` (see proof below). No production logic changed.
- `src/web_service/handlers.rs`: `skeleton_dbnums` and `append_cold_read_through_dbnums` take `&[direct_tree::TreeDatabase]` instead of `&[mdb_memory_store::LoadedDb]`; the two unit tests that build fixtures for them construct `TreeDatabase`. `/dbnums` output is unchanged (same fields, same source values). `health` untouched.
- `tests/refno_native_guards.rs`: three guards re-shaped to the shared read view, intent kept, plus the new "no skeleton residue" assertion. Baseline before the change (`shared-read-guards-before.log`): exactly these three red, 9 green.

## Guard changes, one by one

1. `watcher_and_health_do_not_rebuild_or_require_an_element_skeleton`
   - `health.contains("cache_stats().await")` → `health.contains("mdb_read_store::cache_stats(")`: the `/health` core section reads the shared read view's observation port (no provider `.get().await`, which would open a service).
   - `function_body(ELEMENT_ATTRIBUTES, "fn membership_fingerprint(")` (function removed by the wiring) → for **both** services, `fn is_current(` contains `self.reader.view()` and `Arc::ptr_eq(` and contains none of `DbFileHeader::read(` / `ReadOnlyEngine::open(` / `mdb_memory_store::`. Freshness is only the shared view's `Arc` identity.
2. `uda_catalogue_and_spool_do_not_depend_on_the_element_skeleton`
   - Old intent: `membership_fingerprint` excluded DICT (`db.styp != DICT_STYP`) so dictionary changes never rebuilt the attribute service.
   - New shape, same intent: freshness = `reader.view()` identity; `view()` only re-checks sources the shared `DbSet` has **already opened** (`opened_sources()`), and dictionaries are owned by `UdaCatalogProvider` and never opened through the shared `DbSet`. Asserted as: `is_current` in `element_attributes.rs` contains `self.reader.view()` and none of `catalog` / `DICT` / `dictionary_paths(` / `DbFileHeader::read(`; production `element_attributes.rs` contains neither `DICT_STYP` nor `dictionary_paths(`; production `mdb_read_store.rs` `pub fn view(&self)` contains `opened_sources()` and the module contains neither `DICT_STYP` nor `dictionary_paths(`; `read(` still calls `self.catalog.get()?` exactly once. Why equivalent: the only way a DICT change could invalidate the attribute service is through `is_current`, and `is_current` can only observe files the `DbSet` opened; a module that never enumerates dictionaries cannot open one on its own.
3. `attribute_and_tree_services_take_their_locator_from_the_route_directory`
   - `attributes/tree.contains("MdbRouteDirectory::from_membership(")` → both services contain `MdbReadProvider::shared(`, neither contains `MdbRouteDirectory::from_membership(`; production `mdb_read_store.rs` contains `MdbRouteDirectory::from_membership(` exactly once (`lib.rs::startup_route_directory` keeps its own call for the startup banner; that guard is unchanged).
   - New: production `direct_tree.rs` and `element_attributes.rs` contain no `mdb_memory_store` at all (comments included; the acceptance text asks for `mdb_memory_store::`, this is the stricter superset).
   - Existing `locator_parts()` / `build_from_dir(` / `mdb_memory_store::ensure(` prohibitions kept.

The three assertions that read `mdb_read_store.rs` (T2's file) were reported to T2 (`send_to se-141f0d532dba5372`), file not modified here.

## Sharing proof (acceptance a)

`src/data_interface/element_attributes.rs::tests::ams_tree_and_attribute_services_share_one_read_view`, `#[ignore = "requires local AMS source files"]`, AMS E3D3.1 `/ALL` (current `DbOption.toml`: `store_mode = "spawned-mem"` → read-through, `room_membership = false`). Steps: `read_stats::reset()` → `DirectTreeService::open` → `ElementAttributeService::open` (same `(project, mdb)`) → warm `UdaCatalogProvider` (DICT opens are that provider's, so they are taken before the measured deltas) → `warmed` snapshot → tree `ancestors(24384/23257)` → attribute `read(24384/23257)` → identity/cache/scan assertions.

Assertions, all passed:

- tree read opens the target database: `database_opens` 11 → 12 (+1, DESI 8000);
- attribute service first read of the already-open database: `database_opens` 12 → 12 (+0);
- `response.dbnum == tree.dbnum_of(branch)`;
- `Arc::ptr_eq(&attributes.store, &tree.read_view())` and `attributes.snapshot_epoch == view.epoch` (1);
- `cached_elements` after the tree read = 5; unchanged after the attribute read; unchanged after a second attribute read and a second tree `ancestors`;
- `scan_calls == 0 && scanned_elements == 0` for the whole run.

Literal output line:

```
AMS shared view: ancestors=5 epoch=1 opened_dbnums=[8000] cached_elements=5 database_opens(warm/tree/attributes)=11/12/12 | parse hit=51 shared=2 miss=270 8ms | extract hit=0 shared=7 miss=198 6ms | attr lookups=50 | db opens=12 scans=0 scanned elements=0 point lookups=1616
```

The 11 opens before `warmed` belong to membership resolution (SYS-side files) and the `UdaCatalogProvider` dictionary load; they sit outside the measured deltas by construction. The 1616 point lookups are index reads (dictionary load and element lookups), not element scans — `scan_calls` / `scanned_elements` stayed 0.

## Verification

Toolchain: `cargo 1.99.0-nightly (c79e8f894 2026-08-04)`, `rustc 1.99.0-nightly (1a98b1e13 2026-08-07)`; shared target `D:/Rust/target` (the `Blocking waiting for file lock` lines in the logs are the concurrent T2/T3 builds).

- Baseline before edits: `cargo test --test refno_native_guards -- --test-threads=1` — exit 101; **9 passed, 3 failed** (`attribute_and_tree_services_take_their_locator_from_the_route_directory` at `MdbRouteDirectory::from_membership(`, `uda_catalogue_and_spool_do_not_depend_on_the_element_skeleton` at `missing function: fn membership_fingerprint(`, `watcher_and_health_do_not_rebuild_or_require_an_element_skeleton` at `cache_stats().await`); `shared-read-guards-before.log`.
- `cargo test --test refno_native_guards --test refno_native_api_contract -- --test-threads=1` — exit 0; **12 + 4 passed, 0 failed**; `shared-read-contracts.log`.
- `cargo test --lib ams_tree_and_attribute_services_share_one_read_view -- --ignored --test-threads=1 --nocapture` — exit 0; **1 passed** (1934 filtered out); `shared-read-ams-shared-view.log`.
- `cargo test --lib -- --test-threads=1` — exit 0; **1828 passed, 0 failed, 107 ignored** (baseline 1828 / 0 / 106; the +1 ignored is the new opt-in AMS test); `shared-read-lib-regression.log`.
- Editor diagnostics on the four edited source files: none.

NOT RUN:

- `cargo build --bin aios-database` — not in this task's command list; T3 builds the binary on the shared target. The library compiled in both cfg (integration tests) and cfg(test) (lib tests), which covers every changed line.
- Real HTTP `/health` and `/dbnums` — T3's acceptance; `/dbnums` field set is pinned by `refno_native_api_contract::health_dbnums_and_tree_envelopes_are_frozen` (green).
- `cargo clippy` / `cargo fmt --check` — not in the command list; not run to avoid contending for the shared target while T2/T3 build.
- No `D:/AVEVA/Projects/E3D3.1` source file was modified; the AMS test only reads.

## Artifacts and remaining work

- Evidence logs: `shared-read-guards-before.log`, `shared-read-contracts.log`, `shared-read-ams-shared-view.log`, `shared-read-lib-regression.log` (this batch); `shared-read-check.log` / `shared-read-check-final.log` and `shared-read-original/` are from the wiring step before it.
- Not in this batch (other tasks): per-dbnum invalidation behavior (copy change / corruption / restore) — T2 `tests/shared_read_view_invalidation.rs`; real HTTP attribute → ancestors → children acceptance — T3; `TreeMetadata::open` still initializes metadata for every element database on first `/dbnums` / `roots` (lazy per-dbnum metadata is still open, see `dbset-results.md` tail); SAVEWORK/restore consistency, room-on, JEU and packaging gates remain outstanding. Not committed.
