# 2026-08-24 OCC retire caliber implementation evidence

## Scope and immutable inputs

- Main worktree: `D:\work\plant-code\old\gen-model-occ-retire-endgame`, base `cf7ec05d`.
- aios-core base: `29c91f48ce230814a26466d2150d51385417fab8`.
- Cylinder census input SHA-256:
  - `cyl-diameters.json`: `8D0877F4C8D4180D4FFC2E672E97459E462FB120DECFDFDE5156C56009048FEE`
  - `cyl-diameter-histogram.json`: `60963E0C4B535B1A2DE69D6C53BD3E9614A0A69167D683DFCE45385DD5A4EBCD`
- Existing census has no sphere, SSCL, or polyhedron. No dabacon fixture is present in this worktree, so YOFF source census and T031 stop at the mandatory sample gate.

## Published dependency revisions

```text
aios-core:
a9241e9 feat(mesh): carry physical facet caliber in reusable identities
d4a39c2 fix(snout): preserve normalized two-axis offsets
old-parse-pdms-db: 853ed15 chore(deps): align aios core facet caliber revision
old-pdms-io: 53b9e38 chore(deps): align caliber dependency revisions
```

All four pushes exited 0.

## Verification record

### Vendor identity and normalization

```powershell
cargo test --lib --no-default-features --features gen_model,sql prim_geo::facet_caliber -- --nocapture
```

```text
running 4 tests
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 167 filtered out
exit status: 0
```

```powershell
cargo test --lib --no-default-features --features gen_model,sql prim_geo::snout -- --nocapture
```

```text
running 2 tests
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 169 filtered out
exit status: 0
```

### Main package, local-deps OFF

```powershell
cargo check --no-default-features --features ws,gen_model,manifold,project_hd,http_api
```

```text
Finished dev profile [unoptimized + debuginfo]
exit status: 0
```

```powershell
cargo tree -d | Select-String '^aios_core '
```

```text
NO_DUPLICATE_AIOS_CORE
exit status: 0
```

Targeted regressions:

```text
reusable_surface_calibers_match_the_identity_authority ... ok
reusable_unit_param_without_mesh_caliber_requires_atomic_rebuild ... ok
shared_cylinder_id_has_one_canonical_single_variant_param ... ok
rounded_equal_snout_hashes_produce_one_canonical_param ... ok
```

### Sloped sweep CSG

```powershell
cargo test --locked --lib fast_model::sweep_mesh::tests:: --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture
```

```text
test result: ok. 32 passed; 0 failed; 0 ignored; 0 measured; 1140 filtered out
exit status: 0
```

The production `RuledSolid` path now computes its start/end extension with
`mitre_extension_reach` + `mitre_extension_length`, extrudes the actual discretized profile,
and trims both working planes through Manifold CSG. The regression set includes corresponding
45-degree end cuts, a 60-degree start cut, parallel-plane suppression, and a zero-normal
fail-closed check before Manifold can receive NaN.

Source guards:

```text
production_geometry_does_not_reintroduce_retired_core3d_operations ... ok
catalogue_negatives_have_only_the_manifold_entry ... ok
libgm_receipt_is_derived_only_from_a_valid_plant_mesh ... ok
test result: ok. 3 passed; exit status: 0
```

### CI-shaped main gates

```powershell
cargo fmt --all -- --check
cargo check --locked --no-default-features --features ws,gen_model,manifold,project_hd,http_api
cargo test --locked --lib --no-default-features --features ws,gen_model,manifold,project_hd -- --nocapture
```

```text
fmt exit status: 0
check exit status: 0
lib: 1086 passed; 1 failed; 85 ignored; exit status: 101
failure: data_interface::cata_closure::tests::locator_scan_failure_is_a_result_and_cannot_cache_an_empty_success
panic: src/data_interface/cata_closure.rs:2422:74 called Option::unwrap() on a None value
```

The same untouched test failed before this implementation and also fails in isolation; it is
recorded rather than hidden by the new geometry results.

```text
db8000_two_delete_fixture: 6 passed; exit status 0
db_session_fixture_selfcheck: 15 passed; exit status 0
db8000_session_pairs: 21 passed; exit status 0
pdms_record_boundary: 3 passed; exit status 0
```

### Grounding review

```text
sigmap verify-plan docs/plans/2026-08-24-occ-retire-endgame-plan.md
0 errors; 1 broad-scope warning; exit status 0

sigmap verify-ai-output .context/occ-retire-implementation-answer.md
no hallucinations detected; exit status 0

sigmap review-pr --staged
17 files inspected; 5 missing-test findings; exit status 1
```

The staged review expects separate test files and therefore does not associate the inline Rust
`#[cfg(test)]` regressions with the five edited production modules. Those inline tests are covered
by the targeted and full-lib commands above.

## Stop condition

T031, release workflow removal, maintenance-window rebuild, and T046–T049 remain pending. The hard gate requires dabacon samples for sphere, SSCL, polyhedron, and YOFF Snout plus dual-library RVM; current evidence does not satisfy it.
