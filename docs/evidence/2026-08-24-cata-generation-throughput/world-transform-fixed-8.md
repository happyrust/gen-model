# AMS 8000 authoritative world transforms and Plant UI verification

## Fixture and full initialization

- Run: `.scratch/cata-throughput/world-transform-fixed-8`.
- Storage: new independent SurrealDB 2.1.4 RocksDB; no memory store and no
  Legacy path.
- Configuration: local path dependencies, `geometry_permits=8`, no OCC feature.
- Full initialization completion marker was observed; exit status `0`.
- Wall time: `807.8889631s`; process CPU: `1906.546875s`; peak working set:
  `213000192` bytes.
- Final counts at the completion boundary:
  `aabb=5234 geo_relate=9554 geom_error=2593 inst_geo=3606 inst_info=1309
  inst_relate=2681 pe=21950 pending=0 world_trans=0`.
  `pending=0` is only a captured final-state observation, not the timing or
  completion condition.

## Defect and correction

The CATA page prefetch imported `transform::get_world_transforms_many`, whose
local-matrix implementation was not equivalent to the established
persisted/staging-aware `get_world_transform` resolver. It materialized all
1,907 selected `/1RX03-LCT` relations at the origin even though source POS/ORI
were present.

The batch API now lives beside and delegates to the authoritative resolver in
`aios_core::rs_surreal::spatial`; the production import is
`aios_core::get_world_transforms_many`. The full fresh run persisted 1,907/1,907
selected relations with non-zero translations. Literal FTUB probe result:

```text
refno=24384/22403
translation=[-20475.5,-9921.84,600.0]
rotation=[0.6903455,0.15304592,0.6903455,-0.15304592]
aabb.mins=[-20497.264,-9968.515,600.0001]
aabb.maxs=[-20405.34,-9850.47,3400.0]
```

The increased AABB row count (`4759 -> 5234`) is expected: correctly placed
instances now produce distinct spatial receipts instead of sharing collapsed
origin boxes.

## Plant UI verification

Plant UI was launched against this RocksDB and the run-local mesh directory
with `EGUI_INSPECTION=1`. It connected to project `AvevaMarineSample`, reported
three SITE roots, expanded `/1RX03-EQUI`, and displayed ZONE `/1RX03-LCT`.

The first display proved that the starburst/origin collapse was gone, but also
exposed one independent missing global mesh: `geo_relate` linked
`inst_info:2 -> inst_geo:2` (`geo_type=Tubi`) while `2.mesh` was absent. The
normal mesh query only traversed owned `inst_relate -> inst_info` geometry, so
the ownerless global TUBI/BOXI unit parameters were never guaranteed to enter
the file-generation pipeline.

`gen_inst_meshes` now explicitly adds the two global unit identities, with
deduplication, before applying the existing file/cache filter. A forced
on-demand request for `24384/22403` returned:

```json
{"requested_refno":"24384/22403","generation_root":"24384/22402",
 "generation_root_noun":"BRAN","status":"Generated",
 "model_available":true,"model_instance_count":2,
 "generated_instance_count":1}
```

It created `assets/meshes/2.mesh`; SHA-256:
`eae85baaac74cbbfaf096cd73b6edfaa041a00e988c3dc0d93d8c6c377b1590e`.
The second Plant UI display reported `1996` elements and `4879` mesh instances,
then `模型显示完成：1 个目标`, with `ERROR 0`. The model is spatially
distributed rather than collapsed at the origin.

Screenshots:

- Before the TUBI asset correction:
  `.scratch/cata-throughput/world-transform-fixed-8/plant-ui-validation/zone-model.png`
- Final verification:
  `.scratch/cata-throughput/world-transform-fixed-8/plant-ui-validation/zone-model-after-tubi-fix.png`

## Verification records

- `cata_prefetch_uses_authoritative_world_transform_batch`: 1 passed, exit 0.
- `live_batch_preserves_non_identity_ftub_world_pose`: 1 passed, exit 0,
  `2.47s`.
- `fresh_mesh_generation_always_includes_global_tubi_and_boxi_assets`:
  1 passed, exit 0.
- No-OCC debug binary build with
  `ws,gen_model,manifold,project_hd,http_api`: exit 0.
- `cargo fmt --check`: exit 0.
- `sigmap verify-plan specs/027-cata-generation-throughput/plan.md`: exit 0.
- `sigmap verify-ai-output` on the final verification summary: exit 0.
- `sigmap review-pr --base d99fcc8c^`: two inline-test filename heuristics for
  `cata_model.rs` and `mesh_generate.rs`; both changed files contain the
  regression tests listed above, and there were no scope/security findings in
  the bounded two-commit diff.
- Plant UI final display: `ERROR 0`.
- Validation services on ports 8183, 8173 and 5719 were stopped and the ports
  verified closed.
