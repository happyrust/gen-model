# Manifold flat-normal repair plan

## Goal

Make manifold-generated PlantMesh files render the same hard planar faces as E3D,
without breaking later manifold CSG ingest.

## Constitution Check

- No watermark or persistence-state semantics change.
- Geometry failures remain explicit; no error is downgraded.
- The observed dbnum=8000 chevron is captured by a pure regression test.
- Live regeneration and Plant UI evidence are recorded under `docs/evidence/`.

## Tasks

1. Update `src/fast_model/manifold_csg.rs` to emit complete face normals and weld
   duplicated render vertices before CSG ingest.
2. Add the session-239 chevron regression to
   `src/fast_model/manifold_tessellate.rs`.
3. Run focused tests, check, release build, live forced regeneration, Plant UI
   verification, deployment rollback, and final redeployment.
