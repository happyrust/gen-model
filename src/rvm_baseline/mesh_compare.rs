//! Mesh 级对拍：RVM FacetGroup（E3D 三角化）vs gen OCC 网格，双向表面距离。
//!
//! 为什么不逐顶点：E3D 与 OCC 是两套独立三角化器，顶点集不对齐、三角划分不同，
//! 逐顶点 / 逐三角没有共同基准。只能用与三角化无关的**表面距离**
//! （[`crate::fast_model::shared::two_sided_surface_distance`]）。两侧统一到世界 mm。
//!
//! - RVM 侧：rvm-rs 的 [`rvm_rs::export::Tessellate`] 把每个 group 的几何三角化到本地，
//!   再乘 `geometry.transform`（rvm-rs 已把层级烘进单几何变换，同 OBJ 导出口径）并
//!   放大 `M_TO_MM` 到世界 mm。按 group 名归并。
//! - gen 侧（需 `occ`）：从版本库取 `inst_geo.param`，用 `gen_occ_shape` + `gen_occ_mesh`
//!   **就地重建**单位网格（不依赖磁盘 `.mesh`），再乘 `world_trans × inst.transform`。
//!
//! 阈值不是 1mm：曲面墙两侧都是有限三角化，E3D FacetGroup 的弦误差是判定地板，
//! 门限按实测证据定（见 `mesh_wall_live` 测试与 `docs/2026-08-12_live-test-ledger.md`）。

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use parry3d::math::Point;
use parry3d::shape::{TriMesh, TriMeshFlags};

/// rvm-rs 把几何坐标（含 FacetGroup 顶点、geometry.transform 平移）存成米，
/// E3D world 与生成侧都是毫米。与 [`super::import`] 的 `M_TO_MM` 同一口径。
const M_TO_MM: f32 = 1000.0;

/// 世界 mm 三角网格累加器：多几何 / 多实例合成一个网格。
#[derive(Default)]
pub struct MeshAccum {
    verts: Vec<Point<f32>>,
    idx: Vec<[u32; 3]>,
}

impl MeshAccum {
    /// `cols` 是 rvm-rs `Affine3A::to_cols_array()`（列主序 3×3 + 平移）。手算避开
    /// glam 版本冲突：rvm-rs 的 glam 与本仓的 glam 是依赖图里两份不同的 crate。
    fn add_local_triangulation(&mut self, tri: &rvm_rs::export::Triangulation, cols: &[f32; 12]) {
        let base = self.verts.len() as u32;
        for i in (0..tri.vertices.len()).step_by(3) {
            let (vx, vy, vz) = (tri.vertices[i], tri.vertices[i + 1], tri.vertices[i + 2]);
            let wx = (cols[0] * vx + cols[3] * vy + cols[6] * vz + cols[9]) * M_TO_MM;
            let wy = (cols[1] * vx + cols[4] * vy + cols[7] * vz + cols[10]) * M_TO_MM;
            let wz = (cols[2] * vx + cols[5] * vy + cols[8] * vz + cols[11]) * M_TO_MM;
            self.verts.push(Point::new(wx, wy, wz));
        }
        for i in (0..tri.indices.len()).step_by(3) {
            self.idx.push([
                base + tri.indices[i],
                base + tri.indices[i + 1],
                base + tri.indices[i + 2],
            ]);
        }
    }

    /// 顶点已在世界坐标（gen 侧 world_trans × inst.transform 后）。
    fn add_world_points(&mut self, verts: &[Point<f32>], indices: &[u32]) {
        let base = self.verts.len() as u32;
        self.verts.extend_from_slice(verts);
        for chunk in indices.chunks_exact(3) {
            self.idx
                .push([base + chunk[0], base + chunk[1], base + chunk[2]]);
        }
    }

    /// 把一个已在世界坐标的 `TriMesh` 并进累加器（多构件合成 union）。
    fn add_trimesh(&mut self, m: &TriMesh) {
        let base = self.verts.len() as u32;
        self.verts.extend_from_slice(m.vertices());
        for t in m.indices() {
            self.idx.push([base + t[0], base + t[1], base + t[2]]);
        }
    }

    pub fn into_trimesh(self) -> Option<TriMesh> {
        if self.idx.len() < 1 || self.verts.len() < 3 {
            return None;
        }
        Some(TriMesh::with_flags(
            self.verts,
            self.idx,
            TriMeshFlags::empty(),
        ))
    }
}

/// 把多个已在世界坐标的 `TriMesh` 合并成一个（union 对拍用；不做布尔，只堆叠三角）。
pub fn merge_trimeshes(meshes: &[TriMesh]) -> Option<TriMesh> {
    let mut acc = MeshAccum::default();
    for m in meshes {
        acc.add_trimesh(m);
    }
    acc.into_trimesh()
}

// ───────────────────────── RVM 侧（rvm-rs） ─────────────────────────

/// 解析 RVM，按 group 名归并成世界 mm 三角网格。名字重复的 group 视为歧义，剔除。
///
/// group 名就是快照 `RvmMember::name`（命名元素为 E3D NAME，未命名为
/// `<NOUN> <n> of <OWNER>`），对拍侧据此配对。
pub fn rvm_world_meshes_by_name(rvm_path: &Path) -> Result<HashMap<String, TriMesh>> {
    use rvm_rs::parse_rvm;
    use rvm_rs::store::Store;
    use rvm_rs::store::node::{NodeId, NodeKind};

    let bytes = std::fs::read(rvm_path)
        .with_context(|| format!("读取 RVM 失败: {}", rvm_path.display()))?;
    let mut store = Store::new();
    parse_rvm(&bytes, &mut store)
        .with_context(|| format!("解析 RVM 失败: {}", rvm_path.display()))?;

    let mut accum: HashMap<String, MeshAccum> = HashMap::new();
    let mut seen_group: HashSet<String> = HashSet::new();
    let mut ambiguous: HashSet<String> = HashSet::new();

    fn walk(
        store: &Store,
        node_id: NodeId,
        accum: &mut HashMap<String, MeshAccum>,
        seen_group: &mut HashSet<String>,
        ambiguous: &mut HashSet<String>,
    ) {
        let Some(node) = store.get_node(node_id) else {
            return;
        };
        if let NodeKind::Group(group) = &node.kind {
            let name = store.get_string(group.name).trim().to_string();
            if !name.is_empty() {
                // 同名 group 出现两次即歧义（配对不可信），标记后剔除。
                if !seen_group.insert(name.clone()) {
                    ambiguous.insert(name.clone());
                }
                let entry = accum.entry(name).or_default();
                let mut link = group.first_geometry;
                while let Some(gid) = link {
                    if let Some(geometry) = store.get_geometry(gid) {
                        add_geometry(entry, geometry);
                        link = geometry.next;
                    } else {
                        break;
                    }
                }
            }
        }
        // 递归子节点。
        let mut child = node.first_child;
        while let Some(cid) = child {
            let Some(cnode) = store.get_node(cid) else {
                break;
            };
            walk(store, cid, accum, seen_group, ambiguous);
            child = cnode.next;
        }
    }

    for &root in store.roots() {
        walk(&store, root, &mut accum, &mut seen_group, &mut ambiguous);
    }

    let mut out = HashMap::new();
    for (name, mesh) in accum {
        if ambiguous.contains(&name) {
            continue;
        }
        if let Some(tri) = mesh.into_trimesh() {
            out.insert(name, tri);
        }
    }
    Ok(out)
}

fn add_geometry(accum: &mut MeshAccum, geometry: &rvm_rs::store::geometry::Geometry) {
    use rvm_rs::export::Tessellate;
    use rvm_rs::export::tessellator::get_scale;
    use rvm_rs::store::geometry::GeometryKind;

    // Line 是中心线（渲染成 OBJ `l`），没有表面，跳过。
    let scale = get_scale(&geometry.transform.matrix3.into());
    // RVM 顶点是米；1e-3 m = 1mm 弦容差，原语细分足够密，FacetGroup 忽略该参数。
    let tol = 1.0e-3_f32;
    let cols = geometry.transform.to_cols_array();
    let tri = match &geometry.kind {
        GeometryKind::FacetGroup(fg) => fg.tessellate(tol, scale),
        GeometryKind::Cylinder(g) => g.tessellate(tol, scale),
        GeometryKind::Sphere(g) => g.tessellate(tol, scale),
        GeometryKind::Box(g) => g.tessellate(tol, scale),
        GeometryKind::Pyramid(g) => g.tessellate(tol, scale),
        GeometryKind::CircularTorus(g) => g.tessellate(tol, scale),
        GeometryKind::RectangularTorus(g) => g.tessellate(tol, scale),
        GeometryKind::EllipticalDish(g) => g.tessellate(tol, scale),
        GeometryKind::SphericalDish(g) => g.tessellate(tol, scale),
        GeometryKind::Snout(g) => g.tessellate(tol, scale),
        GeometryKind::Line(_) => return,
    };
    accum.add_local_triangulation(&tri, &cols);
}

// ───────────────────────── gen 侧（版本库 + OCC） ─────────────────────────

/// 两三角网格的双向表面距离，对每堵墙给出一行判定。
#[derive(Debug, Clone)]
pub struct MeshPairResult {
    pub label: String,
    pub gen_refno: String,
    pub distance: Option<crate::fast_model::shared::SurfaceDistance>,
    pub rvm_tris: usize,
    pub gen_tris: usize,
    pub note: Option<String>,
}

#[cfg(feature = "occ")]
mod gen_side {
    use super::*;
    use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
    use aios_core::shape::pdms_shape::PlantMesh;
    use glam::{Mat4, Quat, Vec3};
    use std::path::Path;
    use surrealdb::Surreal;
    use surrealdb::engine::any::Any;

    fn vec3_at(v: &serde_json::Value) -> Vec3 {
        Vec3::new(f_at(v, 0), f_at(v, 1), f_at(v, 2))
    }

    fn f_at(v: &serde_json::Value, idx: usize) -> f32 {
        v.get(idx).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32
    }

    fn mat_from_trans(t: &serde_json::Value) -> Mat4 {
        let tr = t.get("translation").cloned().unwrap_or_default();
        let ro = t.get("rotation").cloned().unwrap_or_default();
        let sc = t.get("scale").cloned().unwrap_or_default();
        let translation = vec3_at(&tr);
        let rotation = Quat::from_xyzw(f_at(&ro, 0), f_at(&ro, 1), f_at(&ro, 2), f_at(&ro, 3));
        let scale = if sc.is_array() {
            vec3_at(&sc)
        } else {
            Vec3::ONE
        };
        Mat4::from_scale_rotation_translation(scale, rotation, translation)
    }

    /// 就地重建一个元素的 gen 世界三角网格。geo_hash → param → OCC 形状 → 网格，
    /// 再乘 `world_trans × inst.transform`。`gen_tol` 是 OCC 细分弦容差（mm）。
    pub async fn gen_world_mesh(
        db: &Surreal<Any>,
        pe_key: &str,
        gen_tol: f64,
    ) -> Result<Option<TriMesh>> {
        let sql =
            format!("SELECT world_trans.d AS wt, insts_flat FROM inst_relate WHERE in = {pe_key};");
        let mut resp = db.query(sql).await.context("查询 inst_relate 失败")?;
        let rows: Vec<serde_json::Value> = resp.take(0).context("解析 inst_relate 结果失败")?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let wt = mat_from_trans(row.get("wt").unwrap_or(&serde_json::Value::Null));
        let insts = row
            .get("insts_flat")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        // 每个 geo_hash 的 unit param 只取一次。
        let mut unit_cache: HashMap<String, Option<PlantMesh>> = HashMap::new();
        let mut accum = MeshAccum::default();

        for inst in &insts {
            let Some(hash) = inst.get("geo_hash").and_then(|v| v.as_str()) else {
                continue;
            };
            if !unit_cache.contains_key(hash) {
                let unit = build_unit_mesh(db, hash, gen_tol).await?;
                unit_cache.insert(hash.to_string(), unit);
            }
            let Some(unit) = unit_cache.get(hash).and_then(|m| m.as_ref()) else {
                continue;
            };
            let inst_mat =
                mat_from_trans(inst.get("transform").unwrap_or(&serde_json::Value::Null));
            let world = wt * inst_mat;
            let mut verts = Vec::with_capacity(unit.vertices.len());
            for v in &unit.vertices {
                let w = world.transform_point3(*v);
                verts.push(Point::new(w.x, w.y, w.z));
            }
            accum.add_world_points(&verts, &unit.indices);
        }
        Ok(accum.into_trimesh())
    }

    async fn build_unit_mesh(
        db: &Surreal<Any>,
        geo_hash: &str,
        gen_tol: f64,
    ) -> Result<Option<PlantMesh>> {
        let sql =
            format!("SELECT param FROM inst_geo WHERE record::id(id) = '{geo_hash}' LIMIT 1;");
        let mut resp = db.query(sql).await.context("查询 inst_geo.param 失败")?;
        let rows: Vec<serde_json::Value> = resp.take(0).context("解析 inst_geo.param 失败")?;
        let param_json = rows
            .into_iter()
            .next()
            .and_then(|r| r.get("param").cloned())
            .filter(|v| v.is_object());
        if let Some(param_json) = param_json {
            let param: PdmsGeoParam = serde_json::from_value(param_json)
                .with_context(|| format!("反序列化 PdmsGeoParam 失败 (geo_hash={geo_hash})"))?;
            match param.gen_occ_shape() {
                Ok(shape) => return Ok(Some(PlantMesh::gen_occ_mesh(&shape, gen_tol)?)),
                Err(error) => {
                    eprintln!("geo_hash={geo_hash} gen_occ_shape 失败，回退磁盘 .mesh: {error}");
                }
            }
        }
        // param 为空或建不出形状 → 磁盘 .mesh（布尔/复合结果，如 BEND；CWD=仓库根，
        // meshes_path 默认 assets/meshes）。
        let mesh_path = Path::new("assets/meshes").join(format!("{geo_hash}.mesh"));
        match PlantMesh::des_mesh_file(&mesh_path) {
            Ok(mesh) => Ok(Some(mesh)),
            Err(_) => Ok(None),
        }
    }
}

#[cfg(feature = "occ")]
pub use gen_side::gen_world_mesh;

#[cfg(all(test, feature = "occ"))]
mod mesh_wall_live {
    use super::*;
    use surrealdb::engine::any::connect;
    use surrealdb::opt::auth::Root;

    /// AMS 1112 CWALL `/1RS-WF03-W-C-RR001` 的 4 堵 WALL，RVM FacetGroup vs gen OCC
    /// 网格，双向表面距离。
    ///
    /// 实测结论（2026-08-14，见 `docs/2026-08-12_live-test-ledger.md`）：
    /// - **gen→rvm** 处处贴合：WALL 1/2/3 的 gen 表面几乎整片落在 E3D 面上
    ///   （p95 ≤ ~8mm，仅弦误差量级）。本测试据此对这三堵墙断言 `g2r.p95 ≤ 12mm`
    ///   —— 圆弧墙世界包围盒/几何回归的 mesh 级守卫。
    /// - **rvm→gen** 有约半墙厚（~650mm）的**局部**离群簇：E3D 墙面开了洞
    ///   （WALL 1 FacetGroup polygons=48 / contours=50，2 个内环），gen 的实心
    ///   SweepSolid 不开洞 —— 这是**建模范围差异**，不在本守卫内判红，只打印。
    /// - **WALL 4** 是真差异（浅弧墙厚度朝向：gen→rvm p95≈171、AABB Y 差 ~115mm），
    ///   单列打印，待 SweepSolid 修复后再收进断言。
    ///
    /// 前置：8009 生产验证库在跑；`test_data/rvm/1RS-WF03-W-C-RR001.rvm` 在位。
    /// 跑法：`cargo test --features rvm_verify,occ mesh_wall_surface_distance -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "live 8009 + occ：1112 CWALL WALL 的 mesh 级对拍（gen→rvm 贴合守卫 + 洞/WALL4 取证）"]
    async fn mesh_wall_surface_distance() {
        // RVM 侧：group 名 → 世界 mm 网格。
        let rvm_path = std::path::Path::new("test_data/rvm/1RS-WF03-W-C-RR001.rvm");
        let rvm_meshes = rvm_world_meshes_by_name(rvm_path).expect("parse rvm");

        // gen 侧：连 8009。
        let db = connect("ws://127.0.0.1:8009").await.expect("connect 8009");
        db.signin(Root {
            username: "root",
            password: "root",
        })
        .await
        .expect("signin");
        db.use_ns("1516")
            .use_db("AvevaMarineSample")
            .await
            .expect("use ns/db");

        // WALL n（RVM 名）↔ gen refno（同 rvm_aabb_compare.py 的序号配对）。
        let pairs = [
            ("WALL 1 of CWALL /1RS-WF03-W-C-RR001", "pe:17496_105912"),
            ("WALL 2 of CWALL /1RS-WF03-W-C-RR001", "pe:17496_105930"),
            ("WALL 3 of CWALL /1RS-WF03-W-C-RR001", "pe:17496_105935"),
            ("WALL 4 of CWALL /1RS-WF03-W-C-RR001", "pe:17496_105940"),
        ];

        use crate::fast_model::shared::one_way_surface_distance;
        // gen→rvm 贴合守卫只钉 WALL 1/2/3；WALL 4 是待修真差异，只取证不判红。
        const GEN_TO_RVM_P95_TOL: f32 = 12.0;
        let guarded = ["WALL 1", "WALL 2", "WALL 3"];
        let mut guard_failures = Vec::new();
        for (rvm_name, pe_key) in pairs {
            let rvm = rvm_meshes
                .get(rvm_name)
                .unwrap_or_else(|| panic!("RVM 缺 group {rvm_name}"));
            let gen_mesh = gen_world_mesh(&db, pe_key, 3.0)
                .await
                .expect("gen mesh")
                .unwrap_or_else(|| panic!("gen 缺网格 {pe_key}"));
            let r2g = one_way_surface_distance(rvm, &gen_mesh, 4000).expect("r2g");
            let g2r = one_way_surface_distance(&gen_mesh, rvm, 4000).expect("g2r");
            let ra = rvm.local_aabb();
            let ga = gen_mesh.local_aabb();
            println!(
                "{rvm_name} ({pe_key}): rvm_tris={} gen_tris={}",
                rvm.indices().len(),
                gen_mesh.indices().len()
            );
            println!(
                "    gen->rvm mean={:.2} p95={:.2} max={:.2}  (gen 表面贴 E3D 的程度)",
                g2r.mean, g2r.p95, g2r.hausdorff
            );
            println!(
                "    rvm->gen mean={:.2} p95={:.2} max={:.2}  (E3D 有、gen 无 → 洞/范围差)",
                r2g.mean, r2g.p95, r2g.hausdorff
            );
            println!(
                "    rvm_aabb min=[{:.1},{:.1},{:.1}] max=[{:.1},{:.1},{:.1}]",
                ra.mins.x, ra.mins.y, ra.mins.z, ra.maxs.x, ra.maxs.y, ra.maxs.z
            );
            println!(
                "    gen_aabb min=[{:.1},{:.1},{:.1}] max=[{:.1},{:.1},{:.1}]",
                ga.mins.x, ga.mins.y, ga.mins.z, ga.maxs.x, ga.maxs.y, ga.maxs.z
            );
            for (p, dist) in
                crate::fast_model::shared::farthest_from_surface(rvm, &gen_mesh, 4000, 3)
            {
                println!(
                    "    worst rvm->gen [{:.0},{:.0},{:.0}] d={:.1}",
                    p[0], p[1], p[2], dist
                );
            }
            for (p, dist) in
                crate::fast_model::shared::farthest_from_surface(&gen_mesh, rvm, 4000, 3)
            {
                println!(
                    "    worst gen->rvm [{:.0},{:.0},{:.0}] d={:.1}",
                    p[0], p[1], p[2], dist
                );
            }
            // 径向范围（绕弧心 = 原点）：厚度/半径摆放是否一致。
            let radial = |m: &TriMesh| {
                let mut lo = f32::MAX;
                let mut hi = 0.0_f32;
                for v in m.vertices() {
                    let r = (v.x * v.x + v.y * v.y).sqrt();
                    lo = lo.min(r);
                    hi = hi.max(r);
                }
                (lo, hi)
            };
            let (rl, rh) = radial(rvm);
            let (gl, gh) = radial(&gen_mesh);
            println!(
                "    radial rvm=[{:.0},{:.0}] gen=[{:.0},{:.0}]",
                rl, rh, gl, gh
            );
            // 绕世界弧心（≈原点）的角度跨度与 Z 跨度：判「扫掠角差」还是「端面斜接」。
            let ang_z = |m: &TriMesh| {
                let mut alo = f32::MAX;
                let mut ahi = f32::MIN;
                let mut zlo = f32::MAX;
                let mut zhi = f32::MIN;
                for v in m.vertices() {
                    let a = v.y.atan2(v.x).to_degrees();
                    alo = alo.min(a);
                    ahi = ahi.max(a);
                    zlo = zlo.min(v.z);
                    zhi = zhi.max(v.z);
                }
                (alo, ahi, zlo, zhi)
            };
            let (ral, rah, rzl, rzh) = ang_z(rvm);
            let (gal, gah, gzl, gzh) = ang_z(&gen_mesh);
            println!(
                "    angle rvm=[{:.2},{:.2}]({:.2}) gen=[{:.2},{:.2}]({:.2}) | z rvm=[{:.0},{:.0}] gen=[{:.0},{:.0}]",
                ral,
                rah,
                rah - ral,
                gal,
                gah,
                gah - gal,
                rzl,
                rzh,
                gzl,
                gzh
            );
            if guarded.iter().any(|g| rvm_name.starts_with(g)) && g2r.p95 > GEN_TO_RVM_P95_TOL {
                guard_failures.push(format!("{rvm_name}: gen->rvm p95={:.2}", g2r.p95));
            }
        }
        assert!(
            guard_failures.is_empty(),
            "WALL 1/2/3 的 gen 表面必须贴 E3D（gen->rvm p95 ≤ {GEN_TO_RVM_P95_TOL}mm）：{guard_failures:?}"
        );
    }

    /// AMS 8000 `/C-OR-1R345-C` 管系（FTUB 直管 / BEND 弯头）的 mesh 级对拍。
    ///
    /// 目的：AABB 对拍里 2 个 BEND 一直 FAIL（弯头几何存疑）。mesh 级双向表面距离
    /// 用来**定性**——BEND 是真 gen 缺陷（gen→rvm 大 = gen 面偏离 E3D），还是像墙那样
    /// 只是 E3D 侧附加/口径差。FTUB 作对照（AABB 一向过）。第一遍取证，不硬断言。
    ///
    /// 前置：8009 上有 dbnum 8000 的生成几何；`test_data/rvm/C-OR-1R345-C.rvm` 在位。
    /// 跑法：`cargo test --features rvm_verify --lib mesh_pipe_surface_distance -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "live 8009 + occ：8000 C-OR 管系 FTUB/BEND 的 mesh 级对拍（BEND 缺陷定性）"]
    async fn mesh_pipe_surface_distance() {
        use crate::fast_model::shared::{farthest_from_surface, one_way_surface_distance};

        let rvm_path = std::path::Path::new("test_data/rvm/C-OR-1R345-C.rvm");
        let rvm_meshes = rvm_world_meshes_by_name(rvm_path).expect("parse rvm");

        let db = connect("ws://127.0.0.1:8009").await.expect("connect 8009");
        db.signin(Root {
            username: "root",
            password: "root",
        })
        .await
        .expect("signin");
        db.use_ns("1516")
            .use_db("AvevaMarineSample")
            .await
            .expect("use ns/db");

        // FTUBE/BEND n（RVM 名前缀）↔ gen refno（同 rvm_aabb_compare.py c-or-1r345-c）。
        let pairs = [
            ("FTUBE 1", "pe:24384_23258"),
            ("BEND 1", "pe:24384_23259"),
            ("FTUBE 2", "pe:24384_23260"),
            ("FTUBE 3", "pe:24384_23261"),
            ("FTUBE 4", "pe:24384_23262"),
            ("BEND 2", "pe:24384_23263"),
            ("FTUBE 5", "pe:24384_23264"),
            ("FTUBE 6", "pe:24384_23265"),
            ("FTUBE 7", "pe:24384_23266"),
        ];

        // RVM group 全名形如 "FTUBE 1 of ..."：按前缀（后跟空格或全等）匹配。
        let find_mesh = |prefix: &str| {
            let needle = format!("{prefix} ");
            rvm_meshes
                .iter()
                .find(|(name, _)| name.as_str() == prefix || name.starts_with(&needle))
                .map(|(_, m)| m)
        };

        for (rvm_prefix, pe_key) in pairs {
            let Some(rvm) = find_mesh(rvm_prefix) else {
                println!("{rvm_prefix}: RVM 无匹配 group，跳过");
                continue;
            };
            let gen_mesh = match gen_world_mesh(&db, pe_key, 2.0).await.expect("gen mesh") {
                Some(m) => m,
                None => {
                    println!("{rvm_prefix} ({pe_key}): gen 无网格（TUBI 隐含直管？），跳过");
                    continue;
                }
            };
            let r2g = one_way_surface_distance(rvm, &gen_mesh, 4000).expect("r2g");
            let g2r = one_way_surface_distance(&gen_mesh, rvm, 4000).expect("g2r");
            println!(
                "{rvm_prefix} ({pe_key}): rvm_tris={} gen_tris={}",
                rvm.indices().len(),
                gen_mesh.indices().len()
            );
            println!(
                "    gen->rvm mean={:.2} p95={:.2} max={:.2}  (gen 面贴 E3D 的程度)",
                g2r.mean, g2r.p95, g2r.hausdorff
            );
            println!(
                "    rvm->gen mean={:.2} p95={:.2} max={:.2}  (E3D 有、gen 无)",
                r2g.mean, r2g.p95, r2g.hausdorff
            );
            for (p, dist) in farthest_from_surface(&gen_mesh, rvm, 4000, 2) {
                println!(
                    "    worst gen->rvm [{:.0},{:.0},{:.0}] d={:.1}",
                    p[0], p[1], p[2], dist
                );
            }
        }
    }

    /// 装配 union 验证：BEND 1 + 相邻 FTUBE 1/2 合并成一个网格，gen union vs E3D union。
    ///
    /// 判「弯头腿伸进相邻直管」是**装配无害**（union 双向都小 → 腿与直管重叠、最终外观不变）
    /// 还是**真缺陷**（gen→rvm 仍大 → 腿伸出连相邻直管都盖不住）。
    ///
    /// 跑法：`cargo test --features rvm_verify --lib mesh_branch_union -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "live 8009 + occ：C-OR BEND1+相邻 FTUB 的 union mesh 对拍（重叠是否装配无害）"]
    async fn mesh_branch_union_surface_distance() {
        use crate::fast_model::shared::{farthest_from_surface, two_sided_surface_distance};

        let rvm_path = std::path::Path::new("test_data/rvm/C-OR-1R345-C.rvm");
        let rvm_meshes = rvm_world_meshes_by_name(rvm_path).expect("parse rvm");

        let db = connect("ws://127.0.0.1:8009").await.expect("connect 8009");
        db.signin(Root {
            username: "root",
            password: "root",
        })
        .await
        .expect("signin");
        db.use_ns("1516")
            .use_db("AvevaMarineSample")
            .await
            .expect("use ns/db");

        // BEND 1 夹在 FTUBE 1 与 FTUBE 2 之间（refno 顺序 23258/23259/23260）。
        let members = [
            ("FTUBE 1", "pe:24384_23258"),
            ("BEND 1", "pe:24384_23259"),
            ("FTUBE 2", "pe:24384_23260"),
        ];

        let find_mesh = |prefix: &str| {
            let needle = format!("{prefix} ");
            rvm_meshes
                .iter()
                .find(|(name, _)| name.as_str() == prefix || name.starts_with(&needle))
                .map(|(_, m)| m.clone())
        };

        let mut gen_parts = Vec::new();
        let mut rvm_parts = Vec::new();
        for (prefix, pe_key) in members {
            if let Some(m) = find_mesh(prefix) {
                rvm_parts.push(m);
            }
            if let Some(m) = gen_world_mesh(&db, pe_key, 2.0).await.expect("gen mesh") {
                gen_parts.push(m);
            }
        }
        let rvm_union = merge_trimeshes(&rvm_parts).expect("rvm union");
        let gen_union = merge_trimeshes(&gen_parts).expect("gen union");

        let d = two_sided_surface_distance(&rvm_union, &gen_union, 8000).expect("dist");
        let g2r = crate::fast_model::shared::one_way_surface_distance(&gen_union, &rvm_union, 8000)
            .expect("g2r");
        let r2g = crate::fast_model::shared::one_way_surface_distance(&rvm_union, &gen_union, 8000)
            .expect("r2g");
        println!(
            "UNION(FTUBE1+BEND1+FTUBE2): both mean={:.2} p95={:.2} hausdorff={:.2}",
            d.mean, d.p95, d.hausdorff
        );
        println!(
            "    gen->rvm mean={:.2} p95={:.2} max={:.2} | rvm->gen mean={:.2} p95={:.2} max={:.2}",
            g2r.mean, g2r.p95, g2r.hausdorff, r2g.mean, r2g.p95, r2g.hausdorff
        );
        for (p, dist) in farthest_from_surface(&gen_union, &rvm_union, 8000, 3) {
            println!(
                "    worst gen->rvm [{:.0},{:.0},{:.0}] d={:.1}",
                p[0], p[1], p[2], dist
            );
        }
    }

    /// 端到端：整条 C-OR BRANCH（7 直管 + 2 弯头）合成 union，gen vs E3D。
    ///
    /// 把「gen 装配层几何正确」从 3 元素样本升级为整条管路：union 双向应落在
    /// tessellation 量级（元素边界的弯头腿归属差在整条 union 里自洽抵消）。
    ///
    /// 跑法：`cargo test --features rvm_verify --lib mesh_full_branch -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "live 8009 + occ：整条 C-OR BRANCH 的 union mesh 端到端对拍"]
    async fn mesh_full_branch_union_surface_distance() {
        use crate::fast_model::shared::{farthest_from_surface, two_sided_surface_distance};

        let rvm_path = std::path::Path::new("test_data/rvm/C-OR-1R345-C.rvm");
        let rvm_meshes = rvm_world_meshes_by_name(rvm_path).expect("parse rvm");

        let db = connect("ws://127.0.0.1:8009").await.expect("connect 8009");
        db.signin(Root {
            username: "root",
            password: "root",
        })
        .await
        .expect("signin");
        db.use_ns("1516")
            .use_db("AvevaMarineSample")
            .await
            .expect("use ns/db");

        let members = [
            ("FTUBE 1", "pe:24384_23258"),
            ("BEND 1", "pe:24384_23259"),
            ("FTUBE 2", "pe:24384_23260"),
            ("FTUBE 3", "pe:24384_23261"),
            ("FTUBE 4", "pe:24384_23262"),
            ("BEND 2", "pe:24384_23263"),
            ("FTUBE 5", "pe:24384_23264"),
            ("FTUBE 6", "pe:24384_23265"),
            ("FTUBE 7", "pe:24384_23266"),
        ];

        let find_mesh = |prefix: &str| {
            let needle = format!("{prefix} ");
            rvm_meshes
                .iter()
                .find(|(name, _)| name.as_str() == prefix || name.starts_with(&needle))
                .map(|(_, m)| m.clone())
        };

        let mut gen_parts = Vec::new();
        let mut rvm_parts = Vec::new();
        let mut missing = Vec::new();
        for (prefix, pe_key) in members {
            match find_mesh(prefix) {
                Some(m) => rvm_parts.push(m),
                None => missing.push(format!("rvm:{prefix}")),
            }
            match gen_world_mesh(&db, pe_key, 2.0).await.expect("gen mesh") {
                Some(m) => gen_parts.push(m),
                None => missing.push(format!("gen:{pe_key}")),
            }
        }
        let rvm_union = merge_trimeshes(&rvm_parts).expect("rvm union");
        let gen_union = merge_trimeshes(&gen_parts).expect("gen union");

        let d = two_sided_surface_distance(&rvm_union, &gen_union, 16000).expect("dist");
        let g2r =
            crate::fast_model::shared::one_way_surface_distance(&gen_union, &rvm_union, 16000)
                .expect("g2r");
        let r2g =
            crate::fast_model::shared::one_way_surface_distance(&rvm_union, &gen_union, 16000)
                .expect("r2g");
        println!(
            "FULL BRANCH C-OR ({} gen / {} rvm 构件, missing={:?})",
            gen_parts.len(),
            rvm_parts.len(),
            missing
        );
        println!(
            "    both mean={:.2} p95={:.2} hausdorff={:.2}",
            d.mean, d.p95, d.hausdorff
        );
        println!(
            "    gen->rvm mean={:.2} p95={:.2} max={:.2} | rvm->gen mean={:.2} p95={:.2} max={:.2}",
            g2r.mean, g2r.p95, g2r.hausdorff, r2g.mean, r2g.p95, r2g.hausdorff
        );
        for (p, dist) in farthest_from_surface(&gen_union, &rvm_union, 16000, 3) {
            println!(
                "    worst gen->rvm [{:.0},{:.0},{:.0}] d={:.1}",
                p[0], p[1], p[2], dist
            );
        }
        // 端到端守卫：整条管 union 应落在 tessellation 量级。
        assert!(
            d.p95 <= 10.0 && d.hausdorff <= 30.0,
            "整条 C-OR BRANCH 的 gen union 应贴 E3D union（p95≤10 / max≤30mm）：mean={:.2} p95={:.2} max={:.2}",
            d.mean,
            d.p95,
            d.hausdorff
        );
    }
}
