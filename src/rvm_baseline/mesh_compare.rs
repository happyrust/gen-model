//! Mesh 级对拍：RVM FacetGroup（E3D 三角化）vs gen 侧生产网格，双向表面距离。
//!
//! 为什么不逐顶点：E3D 与本仓是两套独立三角化器，顶点集不对齐、三角划分不同，
//! 逐顶点 / 逐三角没有共同基准。只能用与三角化无关的**表面距离**
//! （[`crate::fast_model::shared::two_sided_surface_distance`]）。两侧统一到世界 mm。
//!
//! - RVM 侧：rvm-rs 的 [`rvm_rs::export::Tessellate`] 把每个 group 的几何三角化到本地，
//!   再乘 `geometry.transform`（rvm-rs 已把层级烘进单几何变换，同 OBJ 导出口径）并
//!   放大 `M_TO_MM` 到世界 mm。按 group 名归并。
//! - gen 侧（需 `manifold`，ADR-030 决策 10 / T043）：有 `booled_id` 时加载布尔后的
//!   `{id}.mesh` 再乘 `world_trans`（与生产 `query_valid_insts` 同口径）；否则从
//!   `inst_geo.param` 就地走生产同款 `tessellate_libgm_param`，再乘
//!   `world_trans × inst.transform`。OCC 只是可选参照分支（带 `occ` 才编译），
//!   拿不到就回退磁盘 `.mesh`——量尺子不再跟被量的对象绑在一起。
//!
//! 阈值不是 1mm：曲面墙两侧都是有限三角化，E3D FacetGroup 的弦误差是判定地板，
//! 门限按实测证据定（见 `mesh_wall_live` 测试与 `docs/2026-08-12_live-test-ledger.md`）。

use std::collections::{HashMap, HashSet};
use std::path::Path;

fn rvm_mesh_dir() -> std::path::PathBuf {
    std::env::var_os("AIOS_RVM_MESH_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("assets/meshes"))
}

use anyhow::{Context, Result};
use parry3d::math::Point;
use parry3d::shape::{TriMesh, TriMeshFlags};

/// rvm-rs 把几何坐标（含 FacetGroup 顶点、geometry.transform 平移）存成米，
/// E3D world 与生成侧都是毫米。与 [`super::import`] 的 `M_TO_MM` 同一口径。
const M_TO_MM: f32 = 1000.0;

/// 与 [`crate::data_interface::staging::query_valid_insts`] 同一口径：
/// `inst_relate.booled_id` 在则只渲染布尔后的那份网格（NXTR/NBOX 已切掉，
/// 实例变换已烘进 `{booled_id}.mesh`）。`NONE` / 空串视为没有。
pub fn resolve_booled_mesh_id(booled_id: Option<&str>) -> Option<&str> {
    let id = booled_id.map(str::trim).filter(|s| !s.is_empty())?;
    if id.eq_ignore_ascii_case("none") {
        return None;
    }
    Some(id)
}

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

// ─────────────────── gen 侧（版本库 + 生产三角化） ───────────────────

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

#[cfg(feature = "manifold")]
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

    /// 与生产 `staging::query_valid_insts` 保持同一可见实例谓词。
    pub(super) fn valid_insts_sql(pe_key: &str) -> String {
        format!(
            r#"SELECT world_trans.d AS wt, booled_id,
                      IF booled_id != NONE {{ [{{ "geo_hash": booled_id }}] }}
                      ELSE {{ (SELECT trans.d AS transform, record::id(out) AS geo_hash
                               FROM out->geo_relate
                               WHERE visible && out.meshed && trans.d != NONE && geo_type = 'Pos') }}
                      AS insts
               FROM {pe_key}->inst_relate
               WHERE aabb.d != NONE AND world_trans.d != NONE;"#
        )
    }

    /// 就地重建一个元素的 gen 世界三角网格。
    ///
    /// 有 `booled_id` 时与生产 `query_valid_insts` 一样，只加载布尔后的
    /// `{booled_id}.mesh` 再乘 `world_trans`（切洞结果已烘进网格）。没有则
    /// geo_hash → param → 生产同款 `tessellate_libgm_param` → 网格，再乘
    /// `world_trans × inst.transform`。如果 libgm 语义实现不支持该参数，
    /// 只允许对已落盘的布尔/复合结果读取 `.mesh`，不切换几何后端。
    pub async fn gen_world_mesh(db: &Surreal<Any>, pe_key: &str) -> Result<Option<TriMesh>> {
        // 走这个 pe 的出边，不要 `WHERE in = {pe_key}`：`inst_relate` 没有
        // `(in, out)` 索引，谓词形式在真库上是整表扫，而对拍要逐件调用本函数。
        let sql = valid_insts_sql(pe_key);
        let mut resp = db.query(sql).await.context("查询 inst_relate 失败")?;
        let rows: Vec<serde_json::Value> = resp.take(0).context("解析 inst_relate 结果失败")?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let wt = mat_from_trans(row.get("wt").unwrap_or(&serde_json::Value::Null));
        let booled_id = row.get("booled_id").and_then(|v| v.as_str());
        if let Some(booled_id) = super::resolve_booled_mesh_id(booled_id) {
            let mesh_path = rvm_mesh_dir().join(format!("{booled_id}.mesh"));
            let unit = PlantMesh::des_mesh_file(&mesh_path).with_context(|| {
                format!(
                    "{pe_key} 有 booled_id={booled_id} 但缺少 {}（布尔网格未落盘，不能回退到未开洞正挤出）",
                    mesh_path.display()
                )
            })?;
            let mut accum = MeshAccum::default();
            let mut verts = Vec::with_capacity(unit.vertices.len());
            for v in &unit.vertices {
                let w = wt.transform_point3(*v);
                verts.push(Point::new(w.x, w.y, w.z));
            }
            accum.add_world_points(&verts, &unit.indices);
            return Ok(accum.into_trimesh());
        }
        let insts = row
            .get("insts")
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
                let unit = build_unit_mesh(db, hash).await?;
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

    async fn build_unit_mesh(db: &Surreal<Any>, geo_hash: &str) -> Result<Option<PlantMesh>> {
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
            // 与生产 `gen_inst_meshes` 同一条路：形状由 `tessellate_libgm_param` 裁决。
            // 写成别的路径，这道 RVM 门量的就是一条生产上不走的路——后端换了它还是
            // 绿的，等于没有门。
            match crate::fast_model::manifold_tessellate::tessellate_libgm_param(&param) {
                Ok(Some(mesh)) => return Ok(Some(mesh)),
                Ok(None) => {}
                Err(error) => {
                    eprintln!("geo_hash={geo_hash} libgm 三角化失败，尝试读取已落盘结果: {error}");
                }
            }
        }
        // param 为空或建不出形状 → 磁盘 .mesh（布尔/复合结果，如 BEND；CWD=仓库根，
        // meshes_path 默认 assets/meshes）。
        let mesh_path = rvm_mesh_dir().join(format!("{geo_hash}.mesh"));
        match PlantMesh::des_mesh_file(&mesh_path) {
            Ok(mesh) => Ok(Some(mesh)),
            Err(_) => Ok(None),
        }
    }
}

#[cfg(feature = "manifold")]
pub use gen_side::gen_world_mesh;

#[cfg(all(test, feature = "manifold"))]
mod mesh_wall_live {
    use super::*;
    use bevy_transform::prelude::Transform;
    use glam::Mat4;
    use surrealdb::engine::any::connect;
    use surrealdb::opt::auth::Root;

    #[test]
    fn gen_world_mesh_query_follows_production_relation_semantics() {
        let sql = super::gen_side::valid_insts_sql("pe:1_2");
        assert!(sql.contains("FROM out->geo_relate"), "{sql}");
        assert!(sql.contains("visible && out.meshed"), "{sql}");
        assert!(sql.contains("trans.d != NONE && geo_type = 'Pos'"), "{sql}");
        assert!(
            sql.contains("aabb.d != NONE AND world_trans.d != NONE"),
            "{sql}"
        );
        assert!(!sql.contains("insts_flat"), "RVM 门不得读取派生平表: {sql}");
    }

    fn live_db_endpoint() -> String {
        std::env::var("AIOS_RVM_DB_ENDPOINT").unwrap_or_else(|_| "ws://127.0.0.1:8009".to_string())
    }

    async fn live_db() -> surrealdb::Surreal<surrealdb::engine::any::Any> {
        let endpoint = live_db_endpoint();
        let db = connect(&endpoint)
            .await
            .unwrap_or_else(|error| panic!("connect {endpoint}: {error}"));
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
        db
    }

    fn transform_mat(transform: &Transform) -> Mat4 {
        Mat4::from_scale_rotation_translation(
            transform.scale,
            transform.rotation,
            transform.translation,
        )
    }

    /// 直接从目标库的 PE/CATA/SPINE 属性走生产解析器，不依赖历史测试专用
    /// `inst_relate`。这条路让两个数据库副本各自证明自己的源属性可生成同一 RVM 曲面。
    async fn source_profile_world_mesh(pe_key: &str) -> Result<Option<TriMesh>> {
        use aios_core::prim_geo::CateBrepShapeMap;

        let refno: aios_core::RefnoEnum = pe_key.trim_start_matches("pe:").into();
        let geom_info = crate::fast_model::resolve_desi_comp(refno, None)
            .await
            .with_context(|| format!("resolve profile catalogue for {pe_key}"))?;
        let shapes = CateBrepShapeMap::new();
        aios_core::prim_geo::profile::create_profile_geos(refno, &geom_info, &shapes)
            .await
            .with_context(|| format!("create profile geometry for {pe_key}"))?;
        let world_transform = aios_core::get_world_transform(refno)
            .await
            .with_context(|| format!("query world transform for {pe_key}"))?
            .ok_or_else(|| anyhow::anyhow!("{pe_key} has no world transform"))?;
        let Some(entries) = shapes.get(&refno) else {
            return Ok(None);
        };

        let mut accum = MeshAccum::default();
        for shape in entries.iter().filter(|shape| shape.visible) {
            if !shape.brep_shape.check_valid() {
                continue;
            }
            let unit_shape = shape.brep_shape.gen_unit_shape();
            let param = unit_shape
                .convert_to_geo_param()
                .ok_or_else(|| anyhow::anyhow!("{pe_key} profile unit shape has no parameter"))?;
            let Some(mesh) = crate::fast_model::manifold_tessellate::tessellate_libgm_param(&param)
                .with_context(|| format!("tessellate source profile for {pe_key}"))?
            else {
                continue;
            };

            // 与 `cata_model::gen_cata_geos` 相同：路径 frame 只贡献旋转/平移，
            // 单位形状的真实尺寸由 BrepShapeTrait::get_trans() 贡献。
            let unit_transform = shape.brep_shape.get_trans();
            let instance_transform = Transform {
                translation: shape.transform.translation
                    + shape.transform.rotation * unit_transform.translation,
                rotation: shape.transform.rotation * unit_transform.rotation,
                scale: unit_transform.scale,
            };
            let matrix = transform_mat(&world_transform) * transform_mat(&instance_transform);
            let vertices = mesh
                .vertices
                .iter()
                .map(|vertex| {
                    let point = matrix.transform_point3(*vertex);
                    Point::new(point.x, point.y, point.z)
                })
                .collect::<Vec<_>>();
            accum.add_world_points(&vertices, &mesh.indices);
        }
        Ok(accum.into_trimesh())
    }

    fn rvm_by_prefix<'a>(
        meshes: &'a HashMap<String, TriMesh>,
        prefix: &str,
    ) -> Option<&'a TriMesh> {
        let needle = format!("{prefix} ");
        meshes
            .iter()
            .find(|(name, _)| name.as_str() == prefix || name.starts_with(&needle))
            .map(|(_, m)| m)
    }

    /// AMS 1112 CWALL `/1RS-WF03-W-C-RR001` 的 4 堵 WALL，RVM FacetGroup vs 生产网格
    /// 网格，双向表面距离。
    ///
    /// 实测结论（2026-08-14，见 `docs/2026-08-12_live-test-ledger.md`）：
    /// - **gen→rvm** 处处贴合：WALL 1/2/3 的 gen 表面几乎整片落在 E3D 面上
    ///   （p95 ≤ ~8mm，仅弦误差量级）。本测试据此对这三堵墙断言 `g2r.p95 ≤ 12mm`
    ///   —— 圆弧墙世界包围盒/几何回归的 mesh 级守卫。
    /// - **rvm→gen** 有约半墙厚（~650mm）的**局部**离群簇：E3D 墙面开了洞
    ///   （WALL 1 FacetGroup polygons=48 / contours=50，2 个内环），gen 的实心
    ///   SweepSolid 不开洞 —— 这是**建模范围差异**，不在本守卫内判红，只打印。
    /// - **WALL 4** 的浅弧起点是斜切平面，不是径向端盖。弧段延伸后按 DRNS 裁切，
    ///   gen→rvm p95 从约 171mm 降到 4.1mm，现与其余三堵墙一起判红。
    ///
    /// 前置：8009 生产验证库在跑；`test_data/rvm/1RS-WF03-W-C-RR001.rvm` 在位。
    /// 跑法（无 occ 口径，T043）：`cargo test --locked --lib --no-default-features
    /// --features ws,gen_model,manifold,project_hd,rvm_verify mesh_wall_surface_distance -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "live database：1112 CWALL WALL 的 mesh 级对拍（默认 8009，可由 AIOS_RVM_DB_ENDPOINT 覆盖）"]
    async fn mesh_wall_surface_distance() {
        // RVM 侧：group 名 → 世界 mm 网格。
        let rvm_path = std::path::Path::new("test_data/rvm/1RS-WF03-W-C-RR001.rvm");
        let rvm_meshes = rvm_world_meshes_by_name(rvm_path).expect("parse rvm");

        // gen 侧：默认连 8009；双库门通过 AIOS_RVM_DB_ENDPOINT 指向隔离副本。
        let db = live_db().await;
        // WALL n（RVM 名）↔ gen refno（同 rvm_aabb_compare.py 的序号配对）。
        let pairs = [
            ("WALL 1 of CWALL /1RS-WF03-W-C-RR001", "pe:17496_105912"),
            ("WALL 2 of CWALL /1RS-WF03-W-C-RR001", "pe:17496_105930"),
            ("WALL 3 of CWALL /1RS-WF03-W-C-RR001", "pe:17496_105935"),
            ("WALL 4 of CWALL /1RS-WF03-W-C-RR001", "pe:17496_105940"),
        ];

        use crate::fast_model::shared::one_way_surface_distance;
        // E3D 的内环/洞只影响 rvm→gen；四堵墙的生产表面都必须贴基准。
        const GEN_TO_RVM_P95_TOL: f32 = 12.0;
        let mut guard_failures = Vec::new();
        for (rvm_name, pe_key) in pairs {
            let rvm = rvm_meshes
                .get(rvm_name)
                .unwrap_or_else(|| panic!("RVM 缺 group {rvm_name}"));
            let gen_mesh = gen_world_mesh(&db, pe_key)
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
            if g2r.p95 > GEN_TO_RVM_P95_TOL {
                guard_failures.push(format!("{rvm_name}: gen->rvm p95={:.2}", g2r.p95));
            }
        }
        assert!(
            guard_failures.is_empty(),
            "WALL 1–4 的 gen 表面必须贴 E3D（gen->rvm p95 ≤ {GEN_TO_RVM_P95_TOL}mm）：{guard_failures:?}"
        );
    }

    /// AMS 1112 CWALL `/1RS-WF03-W-C-RR001` 的 4 堵直线 STWALL。AABB 对拍已 8/8；
    /// 这里钉 mesh：E3D FacetGroup 是 6 面盒（无内环），应双向贴合。
    ///
    /// 跑法（无 occ 口径，T043）：`cargo test --locked --lib --no-default-features
    /// --features ws,gen_model,manifold,project_hd,rvm_verify mesh_stwall_surface_distance -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "live database：1112 CWALL STWALL 的 mesh 级对拍（默认 8009，可覆盖 endpoint）"]
    async fn mesh_stwall_surface_distance() {
        use crate::fast_model::shared::one_way_surface_distance;

        let rvm_path = std::path::Path::new("test_data/rvm/1RS-WF03-W-C-RR001.rvm");
        let rvm_meshes = rvm_world_meshes_by_name(rvm_path).expect("parse rvm");
        let db = live_db().await;

        let pairs = [
            ("STWALL 1 of CWALL /1RS-WF03-W-C-RR001", "pe:17496_105812"),
            ("STWALL 2 of CWALL /1RS-WF03-W-C-RR001", "pe:17496_105813"),
            ("STWALL 3 of CWALL /1RS-WF03-W-C-RR001", "pe:17496_105815"),
            ("STWALL 4 of CWALL /1RS-WF03-W-C-RR001", "pe:17496_105816"),
        ];
        const P95_TOL: f32 = 12.0;
        let mut failures = Vec::new();
        // 先收齐再断言：碰到第一堵没生成的墙就 panic 的话，剩下几堵的实测数字
        // 一个都看不到，而「库里没这件几何」和「几何不贴合」要的是两种处置。
        let mut missing = Vec::new();
        for (rvm_name, pe_key) in pairs {
            let rvm = rvm_meshes
                .get(rvm_name)
                .unwrap_or_else(|| panic!("RVM 缺 group {rvm_name}"));
            let Some(gen_mesh) = gen_world_mesh(&db, pe_key).await.expect("gen mesh") else {
                println!("{rvm_name} ({pe_key}): 库里没有这件的生成几何，跳过对拍");
                missing.push(format!("{rvm_name} ({pe_key})"));
                continue;
            };
            let g2r = one_way_surface_distance(&gen_mesh, rvm, 4000).expect("g2r");
            let r2g = one_way_surface_distance(rvm, &gen_mesh, 4000).expect("r2g");
            println!(
                "{rvm_name} ({pe_key}): rvm_tris={} gen_tris={}",
                rvm.indices().len(),
                gen_mesh.indices().len()
            );
            println!(
                "    gen->rvm mean={:.2} p95={:.2} max={:.2} | rvm->gen mean={:.2} p95={:.2} max={:.2}",
                g2r.mean, g2r.p95, g2r.hausdorff, r2g.mean, r2g.p95, r2g.hausdorff
            );
            let rvm_aabb = rvm.local_aabb();
            let gen_aabb = gen_mesh.local_aabb();
            println!(
                "    rvm_aabb={:?}..{:?} gen_aabb={:?}..{:?}",
                rvm_aabb.mins, rvm_aabb.maxs, gen_aabb.mins, gen_aabb.maxs
            );
            if g2r.p95 > P95_TOL {
                failures.push(format!("{rvm_name}: gen->rvm p95={:.2}", g2r.p95));
            }
            if r2g.p95 > P95_TOL {
                failures.push(format!("{rvm_name}: rvm->gen p95={:.2}", r2g.p95));
            }
        }
        assert!(
            failures.is_empty(),
            "STWALL 直线墙应双向贴合（p95 ≤ {P95_TOL}mm）：{failures:?}"
        );
        assert!(
            missing.is_empty(),
            "这些墙在目标库上没有生成几何，对拍等于没跑——先对 CWALL 做一次定向重生成：{missing:?}"
        );
    }

    /// 不依赖历史派生关系的双副本门：每个副本都从自己的 PE/CATA/SPINE 源属性开始，
    /// 经过 `resolve_desi_comp → create_profile_geos → tessellate_libgm_param` 对拍 RVM。
    #[tokio::test]
    #[ignore = "live database：从源属性对拍 4 WALL + 4 STWALL（DB_OPTION_FILE 选择副本）"]
    async fn mesh_wall_and_stwall_from_source_attributes() {
        use crate::fast_model::shared::one_way_surface_distance;

        aios_core::init_surreal()
            .await
            .expect("connect DB_OPTION_FILE database");
        let rvm_path = std::path::Path::new("test_data/rvm/1RS-WF03-W-C-RR001.rvm");
        let rvm_meshes = rvm_world_meshes_by_name(rvm_path).expect("parse rvm");
        let pairs = [
            (
                "WALL 1 of CWALL /1RS-WF03-W-C-RR001",
                "pe:17496_105912",
                false,
            ),
            (
                "WALL 2 of CWALL /1RS-WF03-W-C-RR001",
                "pe:17496_105930",
                false,
            ),
            (
                "WALL 3 of CWALL /1RS-WF03-W-C-RR001",
                "pe:17496_105935",
                false,
            ),
            (
                "WALL 4 of CWALL /1RS-WF03-W-C-RR001",
                "pe:17496_105940",
                false,
            ),
            (
                "STWALL 1 of CWALL /1RS-WF03-W-C-RR001",
                "pe:17496_105812",
                true,
            ),
            (
                "STWALL 2 of CWALL /1RS-WF03-W-C-RR001",
                "pe:17496_105813",
                true,
            ),
            (
                "STWALL 3 of CWALL /1RS-WF03-W-C-RR001",
                "pe:17496_105815",
                true,
            ),
            (
                "STWALL 4 of CWALL /1RS-WF03-W-C-RR001",
                "pe:17496_105816",
                true,
            ),
        ];
        let mut failures = Vec::new();
        for (rvm_name, pe_key, symmetric) in pairs {
            let rvm = rvm_meshes
                .get(rvm_name)
                .unwrap_or_else(|| panic!("RVM missing group {rvm_name}"));
            let generated = source_profile_world_mesh(pe_key)
                .await
                .unwrap_or_else(|error| panic!("generate {pe_key} from source: {error:#}"))
                .unwrap_or_else(|| panic!("source profile produced no mesh for {pe_key}"));
            let g2r = one_way_surface_distance(&generated, rvm, 4000).expect("gen to rvm");
            let r2g = one_way_surface_distance(rvm, &generated, 4000).expect("rvm to gen");
            println!(
                "{rvm_name}: gen->rvm mean={:.2} p95={:.2} max={:.2} | rvm->gen mean={:.2} p95={:.2} max={:.2}",
                g2r.mean, g2r.p95, g2r.hausdorff, r2g.mean, r2g.p95, r2g.hausdorff
            );
            if g2r.p95 > 12.0 || (symmetric && r2g.p95 > 12.0) {
                failures.push(format!(
                    "{rvm_name}: gen->rvm p95={:.2}, rvm->gen p95={:.2}",
                    g2r.p95, r2g.p95
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "source profile RVM failures: {failures:?}"
        );
    }

    /// AMS 8000 `/C-OR-1R345-C` 管系（FTUB 直管 / BEND 弯头）的 mesh 级对拍。
    ///
    /// 目的：AABB 对拍里 2 个 BEND 一直 FAIL（弯头几何存疑）。mesh 级双向表面距离
    /// 用来**定性**——BEND 是真 gen 缺陷（gen→rvm 大 = gen 面偏离 E3D），还是像墙那样
    /// 只是 E3D 侧附加/口径差。FTUB 作对照（AABB 一向过）。第一遍取证，不硬断言。
    ///
    /// 前置：8009 上有 dbnum 8000 的生成几何；`test_data/rvm/C-OR-1R345-C.rvm` 在位。
    /// 跑法（无 occ 口径，T043）：`cargo test --locked --lib --no-default-features
    /// --features ws,gen_model,manifold,project_hd,rvm_verify mesh_pipe_surface_distance -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "live 8009：8000 C-OR 管系 FTUB/BEND 的 mesh 级对拍（BEND 缺陷定性）"]
    async fn mesh_pipe_surface_distance() {
        use crate::fast_model::shared::{farthest_from_surface, one_way_surface_distance};

        let rvm_path = std::path::Path::new("test_data/rvm/C-OR-1R345-C.rvm");
        let rvm_meshes = rvm_world_meshes_by_name(rvm_path).expect("parse rvm");

        let db = live_db().await;

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
            let gen_mesh = match gen_world_mesh(&db, pe_key).await.expect("gen mesh") {
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
    /// 跑法（无 occ 口径，T043）：`cargo test --locked --lib --no-default-features
    /// --features ws,gen_model,manifold,project_hd,rvm_verify mesh_branch_union -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "live 8009：C-OR BEND1+相邻 FTUB 的 union mesh 对拍（重叠是否装配无害）"]
    async fn mesh_branch_union_surface_distance() {
        use crate::fast_model::shared::{farthest_from_surface, two_sided_surface_distance};

        let rvm_path = std::path::Path::new("test_data/rvm/C-OR-1R345-C.rvm");
        let rvm_meshes = rvm_world_meshes_by_name(rvm_path).expect("parse rvm");

        let db = live_db().await;

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
            if let Some(m) = gen_world_mesh(&db, pe_key).await.expect("gen mesh") {
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
    /// 跑法（无 occ 口径，T043）：`cargo test --locked --lib --no-default-features
    /// --features ws,gen_model,manifold,project_hd,rvm_verify mesh_full_branch -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "live 8009：整条 C-OR BRANCH 的 union mesh 端到端对拍"]
    async fn mesh_full_branch_union_surface_distance() {
        use crate::fast_model::shared::{farthest_from_surface, two_sided_surface_distance};

        let rvm_path = std::path::Path::new("test_data/rvm/C-OR-1R345-C.rvm");
        let rvm_meshes = rvm_world_meshes_by_name(rvm_path).expect("parse rvm");

        let db = live_db().await;

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
            match gen_world_mesh(&db, pe_key).await.expect("gen mesh") {
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

    /// 第二条管系：整条 `/C-IY-1R330-B`（ACP1000 槽盒/托盘，18 直管 + 18 弯头）union。
    /// 确认 C-OR 的「gen 表面贴 E3D」在不同目录截面上仍成立；E3D RVM 含槽体外壳
    /// （比 gen 管段大约 100mm 高），rvm→gen 大是范围差。FTUBE 6 是零长隐含直管，两侧都无表面。
    ///
    /// 跑法（无 occ 口径，T043）：`cargo test --locked --lib --no-default-features
    /// --features ws,gen_model,manifold,project_hd,rvm_verify mesh_c_iy_full_branch -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "live 8009：整条 C-IY BRANCH 的 union mesh 端到端对拍"]
    async fn mesh_c_iy_full_branch_union_surface_distance() {
        use crate::fast_model::shared::{farthest_from_surface, two_sided_surface_distance};

        let rvm_path = std::path::Path::new("test_data/rvm/C-IY-1R330-B.rvm");
        let rvm_meshes = rvm_world_meshes_by_name(rvm_path).expect("parse rvm");
        let db = live_db().await;

        let members = [
            ("FTUBE 1", "pe:24384_22405"),
            ("BEND 1", "pe:24384_22406"),
            ("FTUBE 2", "pe:24384_22407"),
            ("BEND 2", "pe:24384_22408"),
            ("FTUBE 3", "pe:24384_22409"),
            ("BEND 3", "pe:24384_22410"),
            ("FTUBE 4", "pe:24384_22411"),
            ("BEND 4", "pe:24384_22412"),
            ("BEND 5", "pe:24384_22413"),
            ("BEND 6", "pe:24384_22414"),
            ("FTUBE 5", "pe:24384_22415"),
            ("FTUBE 6", "pe:24384_22416"),
            ("BEND 7", "pe:24384_22417"),
            ("BEND 8", "pe:24384_22418"),
            ("BEND 9", "pe:24384_22419"),
            ("FTUBE 7", "pe:24384_22420"),
            ("BEND 10", "pe:24384_22421"),
            ("FTUBE 8", "pe:24384_22422"),
            ("BEND 11", "pe:24384_22423"),
            ("FTUBE 9", "pe:24384_22424"),
            ("BEND 12", "pe:24384_22425"),
            ("FTUBE 10", "pe:24384_22426"),
            ("BEND 13", "pe:24384_22427"),
            ("FTUBE 11", "pe:24384_22428"),
            ("BEND 14", "pe:24384_22429"),
            ("FTUBE 12", "pe:24384_22430"),
            ("BEND 15", "pe:24384_22431"),
            ("FTUBE 13", "pe:24384_22432"),
            ("BEND 16", "pe:24384_22433"),
            ("FTUBE 14", "pe:24384_22434"),
            ("FTUBE 15", "pe:24384_22435"),
            ("FTUBE 16", "pe:24384_22436"),
            ("BEND 17", "pe:24384_22437"),
            ("FTUBE 17", "pe:24384_22438"),
            ("BEND 18", "pe:24384_22439"),
            ("FTUBE 18", "pe:24384_22440"),
        ];

        let mut gen_parts = Vec::new();
        let mut rvm_parts = Vec::new();
        let mut missing = Vec::new();
        let mut skipped = Vec::new();
        for (prefix, pe_key) in members {
            let rvm = rvm_by_prefix(&rvm_meshes, prefix).cloned();
            let generated = gen_world_mesh(&db, pe_key).await.expect("gen mesh");
            match (rvm, generated) {
                (Some(r), Some(g)) => {
                    rvm_parts.push(r);
                    gen_parts.push(g);
                }
                (None, None) => skipped.push(format!("{prefix} ({pe_key})")),
                (None, Some(_)) => missing.push(format!("rvm:{prefix}")),
                (Some(_), None) => missing.push(format!("gen:{pe_key}")),
            }
        }
        println!("C-IY skipped zero-surface members: {skipped:?}");
        assert_eq!(
            skipped,
            ["FTUBE 6 (pe:24384_22416)"],
            "零长隐含直管只应跳过 FTUBE 6；多跳或少跳都要重审"
        );
        assert!(
            missing.is_empty(),
            "C-IY 有表面的构件必须两侧都有网格：missing={missing:?}"
        );
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
            "FULL BRANCH C-IY ({} gen / {} rvm 构件)",
            gen_parts.len(),
            rvm_parts.len()
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
        for (p, dist) in farthest_from_surface(&rvm_union, &gen_union, 16000, 3) {
            println!(
                "    worst rvm->gen [{:.0},{:.0},{:.0}] d={:.1}",
                p[0], p[1], p[2], dist
            );
        }
        // gen 表面贴在 E3D 里（ACP1000 槽盒/托盘：E3D RVM 多出约 100mm 高的槽体，
        // gen 是 Ø50 管段；rvm→gen ~100mm 是表示范围差，不在本守卫内判红）。
        assert!(
            g2r.p95 <= 10.0 && g2r.hausdorff <= 30.0,
            "整条 C-IY BRANCH 的 gen 表面应贴 E3D（gen->rvm p95≤10 / max≤30mm）：mean={:.2} p95={:.2} max={:.2}",
            g2r.mean,
            g2r.p95,
            g2r.hausdorff
        );
    }

    async fn ensure_booled_mesh_files(
        db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
        pe_keys: &[&str],
    ) {
        let mut missing_pe = Vec::new();
        for pe_key in pe_keys {
            let sql = format!("SELECT booled_id FROM {pe_key}->inst_relate;");
            let mut resp = db.query(sql).await.expect("booled_id");
            let rows: Vec<serde_json::Value> = resp.take(0).expect("booled rows");
            let raw = rows
                .first()
                .and_then(|r| r.get("booled_id"))
                .and_then(|v| v.as_str());
            match crate::rvm_baseline::mesh_compare::resolve_booled_mesh_id(raw) {
                Some(id) => {
                    let path = rvm_mesh_dir().join(format!("{id}.mesh"));
                    let usable = path
                        .metadata()
                        .ok()
                        .is_some_and(|m| m.is_file() && m.len() > 256);
                    if usable {
                    } else {
                        println!(
                            "{pe_key} missing or empty {}/{id}.mesh — regen boolean",
                            rvm_mesh_dir().display()
                        );
                        missing_pe.push(*pe_key);
                    }
                }
                None => {}
            }
        }
        if missing_pe.is_empty() {
            return;
        }
        aios_core::init_test_surreal()
            .await
            .expect("init_test_surreal 连接配置库");
        let dir = rvm_mesh_dir();
        let mut mesh_refnos = Vec::new();
        let mut bool_refnos = Vec::new();
        for pe_key in &missing_pe {
            let r: aios_core::RefnoEnum = pe_key.trim_start_matches("pe:").into();
            bool_refnos.push(r);
            mesh_refnos.push(r);
            let negs = aios_core::query_deep_neg_inst_refnos(r)
                .await
                .unwrap_or_else(|e| panic!("query_deep_neg_inst_refnos {pe_key}: {e}"));
            println!("{pe_key} neg insts={}", negs.len());
            mesh_refnos.extend(negs);
        }
        println!(
            "gen_inst_meshes {} refnos (replace) then boolean {}",
            mesh_refnos.len(),
            bool_refnos.len()
        );
        crate::fast_model::mesh_generate::gen_inst_meshes(&mesh_refnos, true, dir.clone())
            .await
            .expect("gen_inst_meshes");
        crate::fast_model::manifold_bool::apply_insts_boolean_manifold(
            &bool_refnos,
            true,
            dir,
            crate::data_interface::geom_error::GeometryFailurePolicy::BestEffortFallback,
        )
        .await
        .expect("apply_insts_boolean_manifold");
    }

    /// AMS 1112 CWALL `/1RS-WF03-W-C-RR001` 的 20 堵 GWALL（挤出）合成 union。
    /// 1:1 按 AABB 中心配对会撞在同一簇上，所以装配层对拍。盒状挤出（≤16 三角）
    /// 钉 gen→rvm p95≤1mm；高面片墙的开洞与带 NXTR 的大体量范围差只打印
    /// （见 `mesh_gwall_extra_against_cwall_union`）。
    ///
    /// 跑法（无 occ 口径，T043）：`cargo test --locked --lib --no-default-features
    /// --features ws,gen_model,manifold,project_hd,rvm_verify mesh_gwall_union -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "live database：1112 CWALL 20 堵 GWALL 的 union mesh 对拍（默认 8009，可覆盖 endpoint）"]
    async fn mesh_gwall_union_surface_distance() {
        use crate::fast_model::shared::{farthest_from_surface, two_sided_surface_distance};

        let rvm_path = std::path::Path::new("test_data/rvm/1RS-WF03-W-C-RR001.rvm");
        let rvm_meshes = rvm_world_meshes_by_name(rvm_path).expect("parse rvm");
        let db = live_db().await;

        let pe_keys = [
            "pe:17496_105817",
            "pe:17496_105823",
            "pe:17496_105828",
            "pe:17496_105880",
            "pe:17496_105950",
            "pe:17496_116530",
            "pe:17496_116549",
            "pe:17496_116569",
            "pe:17496_116956",
            "pe:17496_116970",
            "pe:17496_116993",
            "pe:17496_116999",
            "pe:17496_117038",
            "pe:17496_117043",
            "pe:17496_117050",
            "pe:17496_117202",
            "pe:17496_118100",
            "pe:17496_118130",
            "pe:17496_118163",
            "pe:17496_118174",
        ];
        ensure_booled_mesh_files(&db, &pe_keys).await;

        let mut rvm_parts = Vec::new();
        let mut missing_rvm = Vec::new();
        for n in 1..=20 {
            let prefix = format!("GWALL {n}");
            match rvm_by_prefix(&rvm_meshes, &prefix) {
                Some(m) => rvm_parts.push(m.clone()),
                None => missing_rvm.push(prefix),
            }
        }
        let mut gen_parts = Vec::new();
        let mut missing_gen = Vec::new();
        for pe_key in pe_keys {
            match gen_world_mesh(&db, pe_key).await.expect("gen mesh") {
                Some(m) => gen_parts.push(m),
                None => missing_gen.push(pe_key.to_string()),
            }
        }
        println!(
            "GWALL union: rvm={}/20 gen={}/20 missing_rvm={missing_rvm:?} missing_gen={missing_gen:?}",
            rvm_parts.len(),
            gen_parts.len()
        );
        assert!(
            missing_rvm.is_empty() && missing_gen.is_empty(),
            "20 堵 GWALL 必须两侧都有网格：rvm={missing_rvm:?} gen={missing_gen:?}"
        );

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
            "    both mean={:.2} p95={:.2} hausdorff={:.2}",
            d.mean, d.p95, d.hausdorff
        );
        println!(
            "    gen->rvm mean={:.2} p95={:.2} max={:.2} | rvm->gen mean={:.2} p95={:.2} max={:.2}",
            g2r.mean, g2r.p95, g2r.hausdorff, r2g.mean, r2g.p95, r2g.hausdorff
        );
        for (pe_key, part) in pe_keys.iter().zip(&gen_parts) {
            let one = crate::fast_model::shared::one_way_surface_distance(part, &rvm_union, 2000)
                .expect("g2r part");
            println!(
                "    gen {pe_key} -> rvm_union mean={:.1} p95={:.1} max={:.1} tris={}",
                one.mean,
                one.p95,
                one.hausdorff,
                part.indices().len()
            );
        }
        for n in 1..=20 {
            let prefix = format!("GWALL {n}");
            let Some(part) = rvm_by_prefix(&rvm_meshes, &prefix) else {
                continue;
            };
            let one = crate::fast_model::shared::one_way_surface_distance(part, &gen_union, 2000)
                .expect("r2g part");
            println!(
                "    rvm {prefix} -> gen_union mean={:.1} p95={:.1} max={:.1} tris={}",
                one.mean,
                one.p95,
                one.hausdorff,
                part.indices().len()
            );
        }
        for (p, dist) in farthest_from_surface(&gen_union, &rvm_union, 16000, 3) {
            println!(
                "    worst gen->rvm [{:.0},{:.0},{:.0}] d={:.1}",
                p[0], p[1], p[2], dist
            );
        }
        for (p, dist) in farthest_from_surface(&rvm_union, &gen_union, 16000, 3) {
            println!(
                "    worst rvm->gen [{:.0},{:.0},{:.0}] d={:.1}",
                p[0], p[1], p[2], dist
            );
        }
        // 盒状挤出（≤16 三角）必须贴 E3D；大体量/开洞墙只取证。
        let mut simple_failures = Vec::new();
        for (pe_key, part) in pe_keys.iter().zip(&gen_parts) {
            if part.indices().len() > 16 {
                continue;
            }
            let one = crate::fast_model::shared::one_way_surface_distance(part, &rvm_union, 2000)
                .expect("simple g2r");
            if one.p95 > 1.0 {
                simple_failures.push(format!("{pe_key}: p95={:.2}", one.p95));
            }
        }
        assert!(
            simple_failures.is_empty(),
            "盒状 GWALL（≤16 三角）的 gen 表面必须贴 E3D union（p95≤1mm）：{simple_failures:?}"
        );
    }

    /// 3 堵 GWALL 大体量 gen→rvm 余量：NXTR 已布尔后的对拍。
    ///
    /// `inst_relate.booled_id` 指向切洞后网格；对拍与生产 `query_valid_insts`
    /// 同一口径。缺 `.mesh` 时先 `gen_inst_meshes` + `apply_insts_boolean_manifold`。
    /// OCC 布尔会把 116569 切成空网格，生产不走这条路径。盒状守卫仍在
    /// `mesh_gwall_union_surface_distance`。
    ///
    /// 跑法（无 occ 口径，T043）：`cargo test --locked --lib --no-default-features
    /// --features ws,gen_model,manifold,project_hd,rvm_verify mesh_gwall_extra -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "live database：1112 3 堵大体量 GWALL 的 NXTR/范围差取证（默认 8009，可覆盖 endpoint）"]
    async fn mesh_gwall_extra_against_cwall_union() {
        use crate::fast_model::shared::farthest_from_surface;

        let rvm_path = std::path::Path::new("test_data/rvm/1RS-WF03-W-C-RR001.rvm");
        let rvm_meshes = rvm_world_meshes_by_name(rvm_path).expect("parse rvm");
        let db = live_db().await;
        let extra_keys = ["pe:17496_105828", "pe:17496_105880", "pe:17496_116569"];
        ensure_booled_mesh_files(&db, &extra_keys).await;

        let mut gwall_parts = Vec::new();
        let mut cwall_parts = Vec::new();
        for (noun, n) in [("WALL", 4), ("STWALL", 4), ("GWALL", 20)] {
            for i in 1..=n {
                let prefix = format!("{noun} {i}");
                let Some(m) = rvm_by_prefix(&rvm_meshes, &prefix) else {
                    panic!("missing RVM {prefix}");
                };
                cwall_parts.push(m.clone());
                if noun == "GWALL" {
                    gwall_parts.push(m.clone());
                }
            }
        }
        let gwall_union = merge_trimeshes(&gwall_parts).expect("gwall union");
        let cwall_union = merge_trimeshes(&cwall_parts).expect("cwall union");

        let extras = [
            ("pe:17496_105828", 4usize, 12.0_f32),
            ("pe:17496_105880", 5, 12.0),
            ("pe:17496_116569", 8, 180.0),
        ];
        let mut nxtr_failures = Vec::new();
        let mut dist_failures = Vec::new();
        for (pe_key, expect_nxtr, p95_tol) in extras {
            let nxtr_sql =
                format!("SELECT record::id(id) FROM pe WHERE noun = 'NXTR' AND owner = {pe_key};");
            let mut resp = db.query(nxtr_sql).await.expect("nxtr count");
            let rows: Vec<serde_json::Value> = resp.take(0).expect("nxtr rows");
            let n = rows.len();
            println!("{pe_key} NXTR children={n} (expect ≥ {expect_nxtr})");
            if n < expect_nxtr {
                nxtr_failures.push(format!("{pe_key}: nxtr={n}"));
            }

            let Some(part) = gen_world_mesh(&db, pe_key).await.expect("gen mesh") else {
                panic!("{pe_key} has no gen mesh");
            };
            let aabb = part.local_aabb();
            let g2gwall =
                crate::fast_model::shared::one_way_surface_distance(&part, &gwall_union, 4000)
                    .expect("g2 gwall");
            let g2cwall =
                crate::fast_model::shared::one_way_surface_distance(&part, &cwall_union, 4000)
                    .expect("g2 cwall");
            println!(
                "    mesh_aabb min=[{:.1},{:.1},{:.1}] max=[{:.1},{:.1},{:.1}] tris={}",
                aabb.mins.x,
                aabb.mins.y,
                aabb.mins.z,
                aabb.maxs.x,
                aabb.maxs.y,
                aabb.maxs.z,
                part.indices().len()
            );
            println!(
                "    gen->gwall_union p95={:.1} max={:.1} | gen->cwall_union p95={:.1} max={:.1}",
                g2gwall.p95, g2gwall.hausdorff, g2cwall.p95, g2cwall.hausdorff
            );
            for (p, dist) in farthest_from_surface(&part, &gwall_union, 4000, 2) {
                println!(
                    "    worst gen->gwall [{:.0},{:.0},{:.0}] d={:.1}",
                    p[0], p[1], p[2], dist
                );
            }
            if g2gwall.p95 > p95_tol {
                dist_failures.push(format!(
                    "{pe_key}: gen->gwall p95={:.1} > {p95_tol}",
                    g2gwall.p95
                ));
            }
        }
        assert!(
            nxtr_failures.is_empty(),
            "大体量 GWALL 必须带着 NXTR 负体：{nxtr_failures:?}"
        );
        assert!(
            dist_failures.is_empty(),
            "布尔后 gen 应贴 E3D GWALL union（105828/105880 p95≤12，116569 回归 ≤180）：{dist_failures:?}"
        );
    }
}

#[cfg(test)]
mod mesh_source_tests {
    use super::resolve_booled_mesh_id;

    #[test]
    fn booled_id_matches_query_valid_insts() {
        assert_eq!(
            resolve_booled_mesh_id(Some("17496_105828_716")),
            Some("17496_105828_716")
        );
        assert_eq!(
            resolve_booled_mesh_id(Some("  17496_105828_716  ")),
            Some("17496_105828_716")
        );
        assert_eq!(resolve_booled_mesh_id(None), None);
        assert_eq!(resolve_booled_mesh_id(Some("")), None);
        assert_eq!(resolve_booled_mesh_id(Some("NONE")), None);
        assert_eq!(resolve_booled_mesh_id(Some("none")), None);
    }
}
