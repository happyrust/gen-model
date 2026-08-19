# dbnum=8000 session 239 manifold mesh shading repair

## Diagnosis

- E3D source STRU `24384/24946` and copied STRU `24384/26205` have identical EXTR
  parameters, VERT coordinates, geometry hashes, and world transforms.
- `13314936385496256309.mesh` used the OCC fallback and decoded as 30 vertices,
  16 triangles, and 30 valid normals.
- `15682999992713024124.mesh` used manifold tessellation and decoded as 8 vertices,
  16 triangles, and **0 normals**. Plant UI therefore shaded a planar cap differently
  per triangle. The incremental data and placement were not divergent from E3D.

## Change

- `manifold_to_plant_mesh` expands render triangles and emits one outward face normal
  per render vertex.
- `plant_mesh_to_manifold` welds exact transformed positions before CSG ingest, so the
  flat-shaded representation round-trips as shared manifold topology.

## Literal verification

```text
cargo test ... fast_model::manifold_tessellate::tests
running 12 tests
test ...db8000_chevron_extrusion_has_renderable_flat_normals ... ok
test result: ok. 12 passed; 0 failed
exit=0

cargo test ... fast_model::manifold_csg::tests
running 4 tests
test ...manifold_output_has_flat_normals_and_round_trips ... ok
test result: ok. 4 passed; 0 failed
exit=0

cargo check --locked --no-default-features --features ws,gen_model,manifold,occ,project_hd,http_api
Finished `dev` profile
exit=0

cargo build --release --locked --bin aios-database --no-default-features --features ws,gen_model,manifold,occ,project_hd,http_api
Finished `release` profile
exit=0

POST /api/v1/model/ensure {"refno":"24384/26205","force":true}
status=Generated model_available=true model_instance_count=4 generated_instance_count=4
exit=0

15682999992713024124.mesh verts=48 tris=16
agree=16 oppose=0 deg=0
normals=48 (one normal per vertex)
exit=0

GET /api/v1/dbnums (dbnum=8000)
file_latest_sesno=239 applied_sesno=239 blocked=false
GET /api/v1/health
status=ok model_ready=true staging_windows=0
```

Plant UI live result: `查询到 4 个元素、4 个网格实例` and `模型显示完成：1 个目标`.
The repaired V profiles have uniform planar faces in `plant-ui-verified-after.png`.

## Artifacts

- Modified executable: `D:\work\plant-code\old\test-worklspace\bin\aios-database.exe`
- Patch: `flat-normals.patch`
- Verification record: this file plus `mesh-inspect-after.txt`
- Rollback: `rollback-flat-normals.ps1`
- Original executable: `aios-database.before-flat-normals.exe`
- Before UI: `plant-ui.png`
- After UI: `plant-ui-verified-after.png`

The rollback was executed once and restored SHA-256
`D63EBECA9B11E3216300D606E73F4C67D4C5007E7CDCB15D538E2A08725D9F79`.
The repaired binary was then redeployed with SHA-256
`12E3019453CAB9BE1C0A45D00FFAAF2D8ECBDA64673C60881F6D21174195BCE7`.
