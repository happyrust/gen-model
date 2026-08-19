//! ADR-030 Phase 2：按 libgm `gm_CreateBox` / `gm_CreateCylinder` / `gm_CreateExtrusion`
//! 语义用 manifold-csg 出 `PlantMesh`。
//!
//! 箱与柱走 **单位几何**（与 aios-core `BOX_SHAPE` / `CYLINDER_SHAPE` 同一信封）：
//! 边长 1 的中心立方、半径 0.5 高 1 的圆柱。尺寸进实例变换，不烤进网格。
//! 挤出按参数高度沿 +Z，空轮廓 hard fail；顶点 z 的 FRADIUS 倒角走
//! `wire::gen_polyline_original` 权威离散，不得静默丢弃。

use crate::fast_model::manifold_csg::manifold_to_plant_mesh;
use crate::fast_model::mesh_primitives;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::prim_geo::wire::{CurveType, gen_polyline_original};
use aios_core::shape::pdms_shape::{BrepShapeTrait, PlantMesh, VerifiedShape};
use anyhow::anyhow;
use cavalier_contours::polyline::PlineSource;
use manifold_csg::{CrossSection, Manifold};

/// 相邻点近于这个距离就并成一个点（PDMS 单位 mm，与 `sweep_mesh::POS_EPS` 同一口径）。
const POS_EPS: f64 = 1e-4;

/// 对齐 `Shape::box_centered(1,1,1)`。
pub fn tessellate_unit_box() -> PlantMesh {
    manifold_to_plant_mesh(&Manifold::cube(1.0, 1.0, 1.0, true))
}

/// 对齐 `Shape::cylinder_radius_height(0.5, 1.0)`：底在 z=0，顶在 z=1。
pub fn tessellate_unit_cylinder(circular_segments: i32) -> PlantMesh {
    manifold_to_plant_mesh(&Manifold::cylinder(1.0, 0.5, 0.5, circular_segments, false))
}

/// `gm_CreateExtrusion(profile, height)`：`verts` 每圈是一条轮廓（xy 坐标，z 是
/// FRADIUS 倒角半径）。倒角解释复用 aios-core `wire::gen_polyline_original`——
/// OCC 路径（`gen_occ_wires`）用的同一份权威实现：z>0 的顶点换成圆弧段，再按
/// `chord_tol`（弦高容差）折线化。首环是外轮廓，建不出即失败；后续环是孔，
/// 建不出环的跳过（与 `gen_occ_wires` 同一容错口径），绕向不翻转，靠
/// `FillRule::Positive` 挖孔。
pub fn tessellate_extrusion(
    verts: &[Vec<glam::Vec3>],
    height: f32,
    chord_tol: f64,
) -> anyhow::Result<PlantMesh> {
    if verts.is_empty() || verts[0].len() < 3 {
        anyhow::bail!(
            "empty extrusion (loops={} first_len={})",
            verts.len(),
            verts.first().map(|v| v.len()).unwrap_or(0)
        );
    }
    if height <= f32::EPSILON {
        anyhow::bail!("extrusion height {height} is not positive");
    }
    let mut polygons: Vec<Vec<[f64; 2]>> = Vec::with_capacity(verts.len());
    polygons.push(flatten_extrusion_loop(&verts[0], chord_tol)?);
    for hole in verts.iter().skip(1) {
        let Ok(ring) = flatten_extrusion_loop(hole, chord_tol) else {
            continue;
        };
        polygons.push(ring);
    }
    let section = CrossSection::from_polygons(&polygons);
    if section.is_empty() {
        anyhow::bail!("extrusion cross-section is empty after fill");
    }
    let solid = Manifold::extrude(&section, height as f64);
    if solid.is_empty() || solid.num_tri() == 0 {
        anyhow::bail!("extrusion manifold is empty");
    }
    Ok(manifold_to_plant_mesh(&solid))
}

/// 单条轮廓环：倒角 z → 圆弧 → 按弦高折线化，去掉重复点与收尾闭合点。
fn flatten_extrusion_loop(
    loop_pts: &Vec<glam::Vec3>,
    chord_tol: f64,
) -> anyhow::Result<Vec<[f64; 2]>> {
    let pline = gen_polyline_original(loop_pts)?;
    let tol = if chord_tol > 0.0 { chord_tol } else { 1.0 };
    let flat = pline
        .arcs_to_approx_lines(tol)
        .ok_or_else(|| anyhow!("挤出轮廓弧段折线化失败（容差 {tol}）"))?;
    let mut pts: Vec<[f64; 2]> = Vec::with_capacity(flat.vertex_count());
    for v in flat.iter_vertexes() {
        if pts
            .last()
            .is_some_and(|last| (last[0] - v.x).hypot(last[1] - v.y) < POS_EPS)
        {
            continue;
        }
        pts.push([v.x, v.y]);
    }
    while pts.len() >= 2 {
        let (first, last) = (pts[0], pts[pts.len() - 1]);
        if (first[0] - last[0]).hypot(first[1] - last[1]) < POS_EPS {
            pts.pop();
        } else {
            break;
        }
    }
    if pts.len() < 3 {
        anyhow::bail!("挤出轮廓离散后只剩 {} 个点", pts.len());
    }
    Ok(pts)
}

/// `check_valid()` 放行但仍然出空网格的参数，必须在这里断掉。
///
/// 空网格一路传下去会变成「模型悄悄少了一件」——比报错难查得多。
fn covered(mesh: PlantMesh, what: &str) -> anyhow::Result<Option<PlantMesh>> {
    if mesh.indices.len() < 3 || mesh.vertices.is_empty() {
        return Err(anyhow!(
            "{what} tessellated to an empty mesh (vertices={} indices={})",
            mesh.vertices.len(),
            mesh.indices.len()
        ));
    }
    Ok(Some(mesh))
}

/// 已实现的 libgm 原语返回 `Some`；其余返回 `None`（调用方回退 OCC）。
pub fn tessellate_libgm_param(param: &PdmsGeoParam) -> anyhow::Result<Option<PlantMesh>> {
    match param {
        PdmsGeoParam::PrimBox(b) => {
            if !b.check_valid() {
                return Err(anyhow!("PrimBox size is degenerate"));
            }
            covered(tessellate_unit_box(), "PrimBox")
        }
        PdmsGeoParam::PrimLCylinder(c) => {
            if !c.check_valid() {
                return Err(anyhow!("PrimLCylinder is degenerate"));
            }
            covered(tessellate_unit_cylinder(32), "PrimLCylinder")
        }
        PdmsGeoParam::PrimSCylinder(c) => {
            if !c.check_valid() {
                return Err(anyhow!("PrimSCylinder is degenerate"));
            }
            if c.is_sscl() {
                let r = c.pdia / 2.0;
                let h = c.phei.abs();
                let btm = [
                    c.btm_shear_angles[0].to_radians(),
                    c.btm_shear_angles[1].to_radians(),
                ];
                let top = [
                    c.top_shear_angles[0].to_radians(),
                    c.top_shear_angles[1].to_radians(),
                ];
                return covered(
                    mesh_primitives::gen_slope_ended_cylinder(r, h, btm, top, 32),
                    "PrimSCylinder(SSCL)",
                );
            }
            covered(tessellate_unit_cylinder(32), "PrimSCylinder")
        }
        PdmsGeoParam::PrimExtrusion(e) => {
            if matches!(e.cur_type, CurveType::Spline(_)) {
                // 样条轮廓的权威解释在 OCC 的 `gen_occ_spline_wire`；把控制点当
                // 折线角点是另一个形状，宁可回退也不静默变形。
                return Ok(None);
            }
            // 弦高容差与 OCC 三角化同一口径：`Extrusion::tol()`（千分之一截面半径）。
            Ok(Some(tessellate_extrusion(
                &e.verts,
                e.height,
                e.tol().max(1e-3) as f64,
            )?))
        }
        PdmsGeoParam::PrimSphere(s) => {
            if !s.check_valid() {
                return Err(anyhow!("PrimSphere is degenerate"));
            }
            covered(mesh_primitives::unit_sphere(), "PrimSphere")
        }
        PdmsGeoParam::PrimLSnout(s) => {
            if !s.check_valid() {
                return Err(anyhow!("PrimLSnout is degenerate"));
            }
            let height = (s.ptdi - s.pbdi).abs();
            let r_top = s.ptdm / 2.0;
            let r_bottom = s.pbdm / 2.0;
            covered(
                mesh_primitives::gen_snout(r_bottom, r_top, height, s.poff, 0.0, 32),
                "PrimLSnout",
            )
        }
        PdmsGeoParam::PrimDish(d) => {
            if !d.check_valid() {
                return Err(anyhow!("PrimDish is degenerate"));
            }
            if d.prad > 0.0 {
                covered(
                    mesh_primitives::gen_elliptical_dish(d.pdia, d.pheig, 32),
                    "PrimDish(elliptical)",
                )
            } else {
                covered(
                    mesh_primitives::gen_spherical_dish(d.pdia, d.pheig, 32),
                    "PrimDish(spherical)",
                )
            }
        }
        PdmsGeoParam::PrimCTorus(t) => {
            if !t.check_valid() {
                return Err(anyhow!("PrimCTorus is degenerate"));
            }
            covered(
                mesh_primitives::gen_circular_torus(t.rins, t.rout, t.angle, 32, 16),
                "PrimCTorus",
            )
        }
        PdmsGeoParam::PrimRTorus(t) => {
            if !t.check_valid() {
                return Err(anyhow!("PrimRTorus is degenerate"));
            }
            covered(
                mesh_primitives::gen_rectangular_torus(t.rins, t.rout, t.height, t.angle, 32),
                "PrimRTorus",
            )
        }
        PdmsGeoParam::PrimPyramid(p) => {
            if !p.check_valid() {
                return Err(anyhow!("PrimPyramid is degenerate"));
            }
            let height = (p.ptdi - p.pbdi).abs();
            covered(
                mesh_primitives::gen_pyramid(
                    p.pbbt, p.pcbt, p.pbtp, p.pctp, height, p.pbof, p.pcof,
                ),
                "PrimPyramid",
            )
        }
        PdmsGeoParam::PrimLPyramid(p) => {
            if !p.check_valid() {
                return Err(anyhow!("PrimLPyramid is degenerate"));
            }
            let height = (p.ptdi - p.pbdi).abs();
            covered(
                mesh_primitives::gen_pyramid(
                    p.pbbt, p.pcbt, p.pbtp, p.pctp, height, p.pbof, p.pcof,
                ),
                "PrimLPyramid",
            )
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_core::prim_geo::SBox;
    use glam::Vec3;

    fn assert_solid_mesh(mesh: &PlantMesh) {
        assert!(mesh.indices.len() >= 3, "mesh must have triangles");
        assert!(mesh.vertices.len() >= 3, "mesh must have vertices");
    }

    #[test]
    fn unit_box_is_non_empty() {
        assert_solid_mesh(&tessellate_unit_box());
    }

    #[test]
    fn unit_cylinder_is_non_empty() {
        assert_solid_mesh(&tessellate_unit_cylinder(24));
    }

    #[test]
    fn square_extrusion_is_non_empty() {
        let verts = vec![vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(10.0, 10.0, 0.0),
            Vec3::new(0.0, 10.0, 0.0),
        ]];
        let mesh = tessellate_extrusion(&verts, 5.0, 1.0).expect("square extrusion");
        assert_solid_mesh(&mesh);
        crate::fast_model::mesh_assert::assert_volume(&mesh, 10.0 * 10.0 * 5.0, 1e-4, "square");
    }

    #[test]
    fn db8000_chevron_extrusion_has_renderable_flat_normals() {
        let verts = vec![vec![
            Vec3::new(-100.0, -200.0, 0.0),
            Vec3::new(-3.59, -200.0, 0.0),
            Vec3::new(-62.0, -25.0, 0.0),
            Vec3::new(-3.59, 150.0, 0.0),
            Vec3::new(-100.0, 150.0, 0.0),
        ]];
        let mesh = tessellate_extrusion(&verts, 100.0, 0.35).expect("db8000 chevron extrusion");

        crate::fast_model::mesh_assert::assert_solid_mesh(&mesh, "db8000 chevron");
        for face in mesh.indices.chunks_exact(3) {
            let normal = mesh.normals[face[0] as usize];
            assert!(
                face.iter()
                    .all(|&index| mesh.normals[index as usize].abs_diff_eq(normal, 1e-6))
            );
        }
    }

    #[test]
    fn empty_extrusion_is_hard_fail() {
        let err = tessellate_extrusion(&[], 10.0, 1.0).unwrap_err();
        assert!(err.to_string().contains("empty extrusion"), "{err}");
    }

    /// FRADIUS 倒角（顶点 z）必须离散成圆弧，不得静默变成直角。
    /// 100×100 方截面四角倒 r=20：面积 = 10000 − (4−π)·400。
    #[test]
    fn filleted_extrusion_matches_analytic_volume() {
        let r = 20.0f32;
        let verts = vec![vec![
            Vec3::new(0.0, 0.0, r),
            Vec3::new(100.0, 0.0, r),
            Vec3::new(100.0, 100.0, r),
            Vec3::new(0.0, 100.0, r),
        ]];
        let mesh = tessellate_extrusion(&verts, 10.0, 0.05).expect("filleted extrusion");
        assert_solid_mesh(&mesh);
        let exact = (100.0 * 100.0 - (4.0 - std::f32::consts::PI) * r * r) * 10.0;
        crate::fast_model::mesh_assert::assert_volume(&mesh, exact, 0.01, "filleted extrusion");
    }

    /// 带孔轮廓：后续环靠反绕向被 `FillRule::Positive` 挖掉，倒角修复不得破坏挖孔。
    #[test]
    fn extrusion_hole_loop_still_subtracts() {
        let outer = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(100.0, 0.0, 0.0),
            Vec3::new(100.0, 100.0, 0.0),
            Vec3::new(0.0, 100.0, 0.0),
        ];
        let hole = vec![
            Vec3::new(30.0, 30.0, 0.0),
            Vec3::new(30.0, 70.0, 0.0),
            Vec3::new(70.0, 70.0, 0.0),
            Vec3::new(70.0, 30.0, 0.0),
        ];
        let mesh = tessellate_extrusion(&[outer, hole], 10.0, 1.0).expect("holed extrusion");
        assert_solid_mesh(&mesh);
        let exact = (100.0 * 100.0 - 40.0 * 40.0) * 10.0;
        crate::fast_model::mesh_assert::assert_volume(&mesh, exact, 1e-4, "holed extrusion");
    }

    /// 样条轮廓（`CurveType::Spline`）没有 libgm 等价实现，必须回 `None` 走 OCC。
    #[test]
    fn spline_extrusion_falls_back_to_occ() {
        let param = PdmsGeoParam::PrimExtrusion(aios_core::prim_geo::extrusion::Extrusion {
            verts: vec![vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(10.0, 0.0, 0.0),
                Vec3::new(10.0, 10.0, 0.0),
            ]],
            height: 5.0,
            cur_type: CurveType::Spline(1.0),
        });
        let result = tessellate_libgm_param(&param).expect("spline extrusion is not an error");
        assert!(result.is_none(), "样条轮廓必须回退 OCC，不得折线近似");
    }

    #[test]
    fn prim_box_param_uses_unit_mesh() {
        let param = PdmsGeoParam::PrimBox(SBox {
            center: Vec3::ZERO,
            size: Vec3::new(100.0, 20.0, 8.0),
        });
        let mesh = tessellate_libgm_param(&param)
            .expect("box")
            .expect("covered");
        assert_solid_mesh(&mesh);
    }

    #[test]
    fn unsheared_scylinder_uses_unit_mesh() {
        let param = PdmsGeoParam::PrimSCylinder(aios_core::prim_geo::SCylinder {
            pdia: 200.0,
            phei: 80.0,
            ..Default::default()
        });
        let mesh = tessellate_libgm_param(&param)
            .expect("cyl")
            .expect("unsheared cylinder is gm_CreateCylinder");
        assert_solid_mesh(&mesh);
    }

    #[test]
    fn empty_mesh_is_hard_fail_not_a_silent_skip() {
        let empty = PlantMesh {
            vertices: vec![],
            normals: vec![],
            indices: vec![],
            wire_vertices: vec![],
            aabb: None,
        };
        let err = covered(empty, "PrimProbe").unwrap_err();
        assert!(err.to_string().contains("empty mesh"), "{err}");
    }

    #[test]
    fn sheared_scylinder_uses_mesh_primitives() {
        let param = PdmsGeoParam::PrimSCylinder(aios_core::prim_geo::SCylinder {
            pdia: 200.0,
            phei: 80.0,
            btm_shear_angles: [15.0, 0.0],
            ..Default::default()
        });
        let mesh = tessellate_libgm_param(&param)
            .expect("sheared cylinder is valid")
            .expect("slope-ended cylinder is now covered by mesh_primitives");
        assert_solid_mesh(&mesh);
    }
}
