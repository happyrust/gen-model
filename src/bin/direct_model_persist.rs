//! 把 `e3d-model` 直读生成结果落成 Plant UI 现有的 `.mesh + inst_relate`。
//!
//! 这不经 OBJ：同一个 Manifold 实体先变回元素局部系，再走 gen-model
//! 已有的 `manifold_to_plant_mesh` 序列化口径。库侧仍是 Plant UI 已经消费的
//! `inst_relate.booled_id + aabb + world_trans`。未参与布尔的 BOX/CYLI 则复用
//! `inst_geo + geo_relate.transform`，保持旧版 Plant UI 的实例查询协议。
//!
//! **库行由生产那一套渲染器出**：这里只把生成结果摊成
//! [`ProjectedGeometry`](aios_database::fast_model::model_memory_store::ProjectedGeometry)，
//! 交给 `model_db_adapter::render_records` 拼 SQL。这个探针原先自带一份行渲染
//! （`legacy-instanced-v2` 口径：每个元素都写 `inst_info`、`geo_relate` 带 `pts:[]`、
//! `inst_geo` 上多一份 `param.primitive_key`），与生产的行内容本来就不一样，两边各改各的
//! 迟早对不上——取证探针出的行要是与生产不同，它取的证就不是生产的证。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aios_core::shape::pdms_shape::PlantMesh;
use aios_database::fast_model::model_db_adapter;
use aios_database::fast_model::model_memory_store::{ProjectedGeometry, ProjectedShape};
use anyhow::{Context, bail};
use e3d_io::db_element::{DbFilePin, DbSet, template_file_for};
use e3d_io::engine::{ReadOnlyEngine, ScanTier};
use e3d_io::refno::RefNo;
use e3d_model::db_discovery::DirectoryDbResolver;
use e3d_model::elmodl::GeometryId;
use e3d_model::pipeline::{Report, generate_subtree};
use e3d_model::primitive_instance::{PrimitiveMeshKey, canonical_primitive_mesh};
use e3d_model::transform::dmat4_to_affine4x3;
use serde::Serialize;

#[derive(Debug)]
struct Args {
    attlib: PathBuf,
    db_file: PathBuf,
    template: PathBuf,
    mesh_dir: PathBuf,
    evidence_dir: PathBuf,
    db_type: String,
    sesno: Option<u32>,
    root: Option<RefNo>,
    catalogues: Vec<PathBuf>,
    project_dirs: Vec<PathBuf>,
    persist: bool,
}

#[derive(Debug, Serialize)]
struct PersistedElement {
    geometry_id: GeometryId,
    refno: String,
    noun: String,
    mesh_id: String,
    storage: &'static str,
    primitive_key: Option<PrimitiveMeshKey>,
    mesh_path: String,
    vertices: usize,
    triangles: usize,
    local_aabb: [[f32; 3]; 2],
    world_aabb: [[f32; 3]; 2],
    transform_id: String,
    aabb_id: String,
    anc: Vec<i64>,
}

#[derive(Debug)]
struct MeshArtifact {
    id: String,
    mesh: PlantMesh,
    path: PathBuf,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut attlib = None;
    let mut db_file = None;
    let mut template = None;
    let mut mesh_dir = None;
    let mut evidence_dir = None;
    let mut db_type = "DESI".to_string();
    let mut sesno = None;
    let mut root = None;
    let mut catalogues = Vec::new();
    let mut project_dirs = Vec::new();
    let mut persist = false;
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().with_context(|| format!("参数 {flag} 缺值"));
        match flag.as_str() {
            "--attlib" => attlib = Some(PathBuf::from(value()?)),
            "--db" => db_file = Some(PathBuf::from(value()?)),
            "--template" => template = Some(PathBuf::from(value()?)),
            "--mesh-dir" => mesh_dir = Some(PathBuf::from(value()?)),
            "--evidence-dir" => evidence_dir = Some(PathBuf::from(value()?)),
            "--db-type" => db_type = value()?,
            "--sesno" => sesno = Some(value()?.parse().context("--sesno 要无符号整数")?),
            "--root" => {
                let raw = value()?;
                let (word0, word1) = raw
                    .trim_start_matches('=')
                    .split_once('/')
                    .with_context(|| format!("--root 要 w0/w1，得到 {raw}"))?;
                root = Some(RefNo::new(word0.parse()?, word1.parse()?));
            }
            "--catalogue" => catalogues.push(PathBuf::from(value()?)),
            "--project-dir" => project_dirs.push(PathBuf::from(value()?)),
            "--persist" => persist = true,
            other => bail!("不认识的参数 {other}"),
        }
    }
    Ok(Args {
        attlib: attlib.context("缺 --attlib")?,
        db_file: db_file.context("缺 --db")?,
        template: template.context("缺 --template")?,
        mesh_dir: mesh_dir.context("缺 --mesh-dir")?,
        evidence_dir: evidence_dir.context("缺 --evidence-dir")?,
        db_type,
        sesno,
        root,
        catalogues,
        project_dirs,
        persist,
    })
}

#[derive(Debug)]
struct ScanIndex {
    roots: Vec<RefNo>,
    owners: BTreeMap<(u32, u32), RefNo>,
}

fn scan_index(path: &Path, sesno: Option<u32>) -> anyhow::Result<ScanIndex> {
    let mut engine = match sesno {
        Some(value) => ReadOnlyEngine::open_at(path, value),
        None => ReadOnlyEngine::open(path),
    }?;
    let mut indexed = BTreeSet::new();
    let mut owners = BTreeMap::new();
    for element in engine.scan_elements(ScanTier::Header)? {
        let element = element?;
        indexed.insert((element.refno.word0, element.refno.word1));
        owners.insert((element.refno.word0, element.refno.word1), element.owner);
    }
    let roots = owners
        .iter()
        .filter_map(|(&(word0, word1), &owner)| {
            let refno = RefNo::new(word0, word1);
            (owner == refno || !owner.is_valid() || !indexed.contains(&(owner.word0, owner.word1)))
                .then_some(refno)
        })
        .collect();
    Ok(ScanIndex { roots, owners })
}

/// Plant UI 的旧层级查询协议：自身 -> OWNER -> ...，只保留源库中真实存在的元素。
/// 值采用 `RefU64` 的 high32/low32 打包口径，直接写纯数字字面量，避免依赖目标库函数。
fn ancestor_chain(refno: RefNo, owners: &BTreeMap<(u32, u32), RefNo>) -> anyhow::Result<Vec<i64>> {
    let mut chain = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = refno;
    loop {
        let current_key = (current.word0, current.word1);
        if !seen.insert(current_key) {
            bail!("OWNER 链成环于 {current}");
        }
        let packed = ((current.word0 as u64) << 32) | current.word1 as u64;
        let packed = i64::try_from(packed)
            .with_context(|| format!("{current} 超出 SurrealDB i64 RefU64 范围"))?;
        chain.push(packed);
        let Some(&owner) = owners.get(&current_key) else {
            break;
        };
        if owner == current
            || !owner.is_valid()
            || !owners.contains_key(&(owner.word0, owner.word1))
        {
            break;
        }
        current = owner;
    }
    Ok(chain)
}

fn mesh_identity(mesh: &PlantMesh) -> String {
    aios_database::fast_model::e3d_mesh_store::baked_mesh_id(mesh)
}

/// 旧格式的 inst_geo id 是 mesh 文件名，必须只由规范网格身份决定。
fn shared_mesh_identity(key: PrimitiveMeshKey, mesh: &PlantMesh) -> String {
    aios_database::fast_model::e3d_mesh_store::canonical_mesh_id(key, mesh)
}

fn geometry_record_raw_id(geometry_id: &GeometryId, source_refno: RefNo) -> String {
    aios_database::fast_model::e3d_mesh_store::geometry_record_id(geometry_id, source_refno)
}

/// 内容寻址 mesh 的跨进程写入：胜者原子发布，失败方校验胜者后清理临时文件。
/// 返回 true 表示本调用写入了新文件，false 表示复用了完整一致的现有文件。
fn write_and_verify_mesh(path: &Path, mesh: &PlantMesh) -> anyhow::Result<bool> {
    Ok(matches!(
        aios_database::fast_model::e3d_mesh_store::ensure_mesh_file(path, mesh)?,
        aios_database::fast_model::e3d_mesh_store::MeshWrite::Written
    ))
}

fn point3_array(point: &parry3d::math::Point<f32>) -> [f32; 3] {
    [point.x, point.y, point.z]
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    std::fs::create_dir_all(&args.mesh_dir)?;
    std::fs::create_dir_all(&args.evidence_dir)?;

    let template = if args.template.is_dir() {
        template_file_for(&args.template, &args.db_type)?
    } else {
        args.template.clone()
    };
    let set = if args.project_dirs.is_empty() {
        Arc::new(DbSet::with_attlib_file(&args.attlib)?)
    } else {
        let template_dir = if args.template.is_dir() {
            args.template.as_path()
        } else {
            args.template.parent().unwrap_or(Path::new("."))
        };
        let resolver = DirectoryDbResolver::scan(&args.project_dirs, template_dir)?;
        println!("{}", resolver.summary());
        Arc::new(DbSet::with_resolver(
            Arc::new(e3d_attlib::AttlibData::parse_file(&args.attlib).map_err(anyhow::Error::msg)?),
            Box::new(resolver),
        ))
    };
    let dbno = set.add_db(DbFilePin {
        file: args.db_file.clone(),
        template,
        db_type: Some(args.db_type.clone()),
        sesno: args.sesno,
    })?;
    let catalogue_template = if args.catalogues.is_empty() {
        None
    } else {
        let template_dir = if args.template.is_dir() {
            args.template.as_path()
        } else {
            args.template.parent().unwrap_or(Path::new("."))
        };
        Some(template_file_for(template_dir, "CATA")?)
    };
    for file in &args.catalogues {
        set.add_db(DbFilePin {
            file: file.clone(),
            template: catalogue_template
                .clone()
                .expect("catalogue template exists when catalogue files are present"),
            db_type: Some("CATA".to_string()),
            sesno: None,
        })?;
    }
    let scan = scan_index(&args.db_file, args.sesno)?;
    let roots = args
        .root
        .map(|root| vec![root])
        .unwrap_or_else(|| scan.roots.clone());
    println!(
        "DIRECT_OPEN dbno={dbno} roots={} catalogues={} project_dirs={}",
        roots.len(),
        args.catalogues.len(),
        args.project_dirs.len()
    );

    let mut report = Report::default();
    let mut generated = Vec::new();
    for root in roots {
        let outcome = generate_subtree(set.element(root))?;
        report.merge(outcome.report);
        generated.extend(outcome.elements);
    }
    println!("{}", report.totals_line());
    std::fs::create_dir_all(&args.evidence_dir)?;
    std::fs::write(
        args.evidence_dir.join("generation-report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;

    let db = if args.persist {
        // 按配置连库：取证要落进服务手上那个库，所以只在 store_mode=rocksdb 下有意义
        // ——mem 档连到的是本进程自己的一份空库，落完就随进程消失，证据等于没留。
        aios_database::attach_store_connected()
            .await
            .context("连接兼容目标 SurrealDB")?;
        Some(aios_core::SUL_DB.clone())
    } else {
        None
    };
    let mut persisted = Vec::with_capacity(generated.len());
    let mut written_shared_meshes = BTreeSet::new();
    let mut mesh_written = 0usize;
    let mut mesh_reused = 0usize;
    // 行由生产渲染器出（`model_db_adapter::render_records`）：探针只把生成结果摊成投影
    // 记录。写入口径因此也是生产那一套（ADR-060 D4）——先按 id 删掉整组表示，再一张表
    // 一条批量 `INSERT IGNORE`。没有 UPDATE / UPSERT，因此也没有「写完再退回去」的
    // 回滚脚本，要退回就重跑一次上一版生成。
    let mut records = Vec::with_capacity(generated.len());
    for element in generated {
        let raw_id = geometry_record_raw_id(&element.geometry_id, element.refno);
        let anc = ancestor_chain(element.refno, &scan.owners)?;
        let world_mesh = element.parts.as_deref().map_or_else(
            || aios_database::fast_model::manifold_csg::manifold_to_plant_mesh(&element.solid),
            aios_database::fast_model::manifold_csg::manifolds_to_plant_mesh,
        );
        let world_aabb = world_mesh.aabb.context("世界系 PlantMesh 没有 AABB")?;
        let world_bounds = [
            point3_array(&world_aabb.mins),
            point3_array(&world_aabb.maxs),
        ];
        // 世界盒与世界变换都按记录 id 做键（ADR-060 D1），两个 id 就是 `raw_id` 本身。
        let world_aabb_id = raw_id.clone();
        let world_transform_id = raw_id.clone();

        let (artifact, storage, primitive_key, local_transform) = if let Some(instance) =
            element.primitive_instance.as_ref()
        {
            let canonical = canonical_primitive_mesh(instance.key)?;
            let mesh = aios_database::fast_model::manifold_csg::manifold_to_plant_mesh(&canonical);
            let mesh_id = shared_mesh_identity(instance.key, &mesh);
            let mesh_path = args.mesh_dir.join(format!("{mesh_id}.mesh"));
            if written_shared_meshes.insert(mesh_id.clone()) {
                if write_and_verify_mesh(&mesh_path, &mesh)? {
                    mesh_written += 1;
                } else {
                    mesh_reused += 1;
                }
            } else {
                mesh_reused += 1;
            }
            (
                MeshArtifact {
                    id: mesh_id,
                    mesh,
                    path: mesh_path,
                },
                "inst_geo",
                Some(instance.key),
                Some(instance.local_transform),
            )
        } else {
            let inverse_world = element.world.inverse();
            let mesh = element.parts.as_deref().map_or_else(
                || {
                    let local = element.solid.transform(&dmat4_to_affine4x3(inverse_world));
                    aios_database::fast_model::manifold_csg::manifold_to_plant_mesh(&local)
                },
                |parts| {
                    aios_database::fast_model::manifold_csg::manifolds_to_transformed_plant_mesh(
                        parts,
                        inverse_world,
                    )
                },
            );
            let mesh_id = mesh_identity(&mesh);
            let mesh_path = args.mesh_dir.join(format!("{mesh_id}.mesh"));
            if write_and_verify_mesh(&mesh_path, &mesh)? {
                mesh_written += 1;
            } else {
                mesh_reused += 1;
            }
            (
                MeshArtifact {
                    id: mesh_id,
                    mesh,
                    path: mesh_path,
                },
                "booled_id",
                None,
                None,
            )
        };
        let mesh = &artifact.mesh;
        if mesh.vertices.len() < 3
            || mesh.indices.len() < 3
            || mesh.normals.len() != mesh.vertices.len()
        {
            bail!("{} {} 转 PlantMesh 后不可显示", element.refno, element.noun);
        }
        let local_aabb = mesh.aabb.context("PlantMesh 没有 AABB")?;
        let local_bounds = [
            point3_array(&local_aabb.mins),
            point3_array(&local_aabb.maxs),
        ];
        records.push(ProjectedGeometry {
            record_id: raw_id,
            geometry_id: element.geometry_id.clone(),
            source_refno: element.refno,
            dbnum: dbno,
            source_sesno: args.sesno.unwrap_or_default(),
            noun: element.noun.clone(),
            mesh_id: artifact.id.clone(),
            world_transform: element.world,
            world_bounds,
            ancestors: anc.clone(),
            shape: match (primitive_key, local_transform) {
                (Some(key), Some(local_transform)) => ProjectedShape::Canonical {
                    key,
                    local_transform,
                    local_bounds,
                },
                _ => ProjectedShape::Baked,
            },
            tube: None,
        });
        persisted.push(PersistedElement {
            geometry_id: element.geometry_id,
            refno: element.refno.to_string(),
            noun: element.noun,
            mesh_id: artifact.id,
            storage,
            primitive_key,
            mesh_path: artifact.path.display().to_string(),
            vertices: mesh.vertices.len(),
            triangles: mesh.indices.len() / 3,
            local_aabb: local_bounds,
            world_aabb: world_bounds,
            transform_id: world_transform_id,
            aabb_id: world_aabb_id,
            anc,
        });
    }
    let update_sql =
        model_db_adapter::render_records(&records, model_db_adapter::WriteMode::Incremental)?;
    std::fs::write(args.evidence_dir.join("update.sql"), &update_sql)?;
    std::fs::write(
        args.evidence_dir.join("persisted-elements.json"),
        serde_json::to_vec_pretty(&persisted)?,
    )?;

    if args.persist {
        let db = db.as_ref().context("持久化模式缺少 SurrealDB 连接")?;
        // 一条批量 INSERT 就是一整行，按行切块不会把它切断。
        for statement_chunk in update_sql.lines().collect::<Vec<_>>().chunks(300) {
            db.query(statement_chunk.join("\n")).await?.check()?;
        }
    }
    println!(
        "DIRECT_PERSIST generated={} unique_meshes={} shared_instances={} baked_instances={} mesh_reused={} mesh_written={} mode={}",
        persisted.len(),
        persisted
            .iter()
            .map(|item| &item.mesh_id)
            .collect::<BTreeSet<_>>()
            .len(),
        persisted
            .iter()
            .filter(|item| item.storage == "inst_geo")
            .count(),
        persisted
            .iter()
            .filter(|item| item.storage == "booled_id")
            .count(),
        mesh_reused,
        mesh_written,
        if args.persist { "persist" } else { "dry-run" }
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn box_mesh() -> PlantMesh {
        let canonical = canonical_primitive_mesh(PrimitiveMeshKey::BoxV1).unwrap();
        aios_database::fast_model::manifold_csg::manifold_to_plant_mesh(&canonical)
    }

    #[test]
    fn shared_identity_is_content_addressed_not_element_addressed() {
        let mesh = box_mesh();
        let first = shared_mesh_identity(PrimitiveMeshKey::BoxV1, &mesh);
        let second = shared_mesh_identity(PrimitiveMeshKey::BoxV1, &mesh);
        assert_eq!(first, second);
        assert!(first.parse::<u64>().is_ok());
        assert_ne!(
            first,
            shared_mesh_identity(PrimitiveMeshKey::CylinderV3 { segments: 16 }, &mesh)
        );
    }

    #[test]
    fn baked_identity_is_exact_content_addressed_and_versioned() {
        let mesh = box_mesh();
        let first = mesh_identity(&mesh);
        let second = mesh_identity(&mesh);
        assert_eq!(first, second);
        assert!(first.starts_with("e3d_baked_v2_"));
        assert_eq!(first.len(), "e3d_baked_v2_".len() + 64);

        let mut changed = mesh.clone();
        changed.normals[0].x = f32::from_bits(changed.normals[0].x.to_bits() ^ 1);
        assert_ne!(first, mesh_identity(&changed));
    }

    #[test]
    fn derived_geometry_ids_are_stable_and_do_not_overwrite_the_container() {
        let source = RefNo::new(17496, 152095);
        let element = GeometryId::Element {
            refno: source.to_string(),
        };
        let tube_a = GeometryId::ImpliedTube {
            container_refno: source.to_string(),
            from_refno: "17496/1".into(),
            to_refno: "17496/2".into(),
            route_ordinal: 0,
        };
        let tube_b = GeometryId::ImpliedTube {
            container_refno: source.to_string(),
            from_refno: "17496/2".into(),
            to_refno: "17496/3".into(),
            route_ordinal: 1,
        };
        assert_eq!(geometry_record_raw_id(&element, source), "17496_152095");
        assert_eq!(
            geometry_record_raw_id(&tube_a, source),
            geometry_record_raw_id(&tube_a, source)
        );
        assert_ne!(
            geometry_record_raw_id(&tube_a, source),
            geometry_record_raw_id(&tube_b, source)
        );
        // 派生 id 是 64 位纯十六进制摘要，与 `<十进制>_<十进制>` 的 refno 形状天生不撞，
        // 所以不戴前缀。
        let derived = geometry_record_raw_id(&tube_a, source);
        assert_eq!(derived.len(), 64);
        assert!(derived.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn content_addressed_file_is_reused_only_after_full_comparison() {
        let dir = tempfile::tempdir().unwrap();
        let mesh = box_mesh();
        let path = dir.path().join(format!("{}.mesh", mesh_identity(&mesh)));
        assert!(write_and_verify_mesh(&path, &mesh).unwrap());
        assert!(!write_and_verify_mesh(&path, &mesh).unwrap());

        let mut collision = mesh.clone();
        collision.indices.swap(0, 1);
        assert!(write_and_verify_mesh(&path, &collision).is_err());
    }

    #[test]
    fn owner_chain_is_self_to_root_and_rejects_cycles() {
        let leaf = RefNo::new(1, 3);
        let parent = RefNo::new(1, 2);
        let root = RefNo::new(1, 1);
        let owners = BTreeMap::from([((1, 3), parent), ((1, 2), root), ((1, 1), root)]);
        assert_eq!(
            ancestor_chain(leaf, &owners).unwrap(),
            vec![4294967299, 4294967298, 4294967297]
        );

        let cyclic = BTreeMap::from([((1, 3), parent), ((1, 2), leaf)]);
        assert!(
            ancestor_chain(leaf, &cyclic)
                .unwrap_err()
                .to_string()
                .contains("成环")
        );
    }

    /// 探针出的行必须与生产逐字同源：这里只摊投影记录，SQL 由
    /// `model_db_adapter::render_records` 出，所以这一条同时钉住「共享图元那条老协议
    /// 在探针这边也成立」与「两边不是各写各的」。
    #[tokio::test]
    async fn legacy_plant_ui_query_resolves_shared_mesh_and_local_transform() {
        let mesh_id = shared_mesh_identity(PrimitiveMeshKey::BoxV1, &box_mesh());
        let record = ProjectedGeometry {
            record_id: "1_2".into(),
            geometry_id: GeometryId::Element {
                refno: "1/2".to_string(),
            },
            source_refno: RefNo::new(1, 2),
            dbnum: 1112,
            source_sesno: 0,
            noun: "BOX".into(),
            mesh_id: mesh_id.clone(),
            world_transform: glam::DMat4::from_scale(glam::DVec3::new(12.0, 34.0, 56.0)),
            world_bounds: [[0.0, 0.0, 0.0], [12.0, 34.0, 56.0]],
            ancestors: vec![4294967298, 4294967297],
            shape: ProjectedShape::Canonical {
                key: PrimitiveMeshKey::BoxV1,
                local_transform: glam::DMat4::from_scale(glam::DVec3::new(12.0, 34.0, 56.0)),
                local_bounds: [[-0.5, -0.5, 0.0], [0.5, 0.5, 1.0]],
            },
            tube: None,
        };
        let shared_sql =
            model_db_adapter::render_records(&[record], model_db_adapter::WriteMode::Incremental)
                .unwrap();
        assert!(shared_sql.contains("booled_id:NONE"));
        assert!(shared_sql.contains("INSERT RELATION IGNORE INTO geo_relate"));
        assert!(shared_sql.contains("anc:["));
        assert!(
            !shared_sql.contains("UPSERT") && !shared_sql.contains("UPDATE"),
            "写入只有先删后插（ADR-060 D4）"
        );

        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem db boots");
        db.use_ns("direct_model_test")
            .use_db("legacy_ui")
            .await
            .unwrap();
        // 先摆一行上一轮的 `inst_relate`：渲染出来的那一段自带「先删后插」，不删的话
        // `INSERT IGNORE` 会撞上同 id 直接跳过，新几何一个字都写不进去。
        let seed = format!(
            "CREATE type::thing('pe','1_2') SET noun='BOX';\n\
             CREATE type::thing('inst_info','old_1_2');\n\
             CREATE type::thing('inst_relate','1_2') SET in=type::thing('pe','1_2'),out=type::thing('inst_info','old_1_2');\n\
             {shared_sql}"
        );
        db.query(seed).await.unwrap().check().unwrap();

        // Keep this selection aligned with old-aios-core::rs_surreal::inst,
        // which is the Plant UI contract for the non-booled branch.
        let query = "SELECT anc, booled_id != NONE AS has_neg, \
            IF booled_id != NONE { [{geo_hash:booled_id}] } ELSE { \
              (SELECT (transform ?? trans.d) AS transform, record::id(out) AS geo_hash \
               FROM out->geo_relate \
               WHERE visible && out.meshed && (transform ?? trans.d) != NONE && geo_type='Pos') \
            } AS insts \
            FROM type::thing('inst_relate','1_2')";
        let mut response = db.query(query).await.unwrap().check().unwrap();
        let rows = response.take::<Vec<Value>>(0).unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0]["anc"], json!([4294967298_i64, 4294967297_i64]));
        assert_eq!(rows[0]["has_neg"], json!(false));
        let instance = &rows[0]["insts"][0];
        assert_eq!(instance["geo_hash"], json!(mesh_id));
        assert_eq!(instance["transform"]["scale"], json!([12.0, 34.0, 56.0]));
    }

    #[tokio::test]
    async fn legacy_plant_ui_query_resolves_content_addressed_baked_mesh() {
        let mesh_id = mesh_identity(&box_mesh());
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem db boots");
        db.use_ns("direct_model_test")
            .use_db("legacy_ui_baked")
            .await
            .unwrap();
        db.query(format!(
            "CREATE type::thing('pe','1_3') SET noun='EXTR';\n\
             CREATE type::thing('inst_relate','1_3') SET in=type::thing('pe','1_3'), \
             booled_id='{mesh_id}', booled=true, anc=[4294967299,4294967297], \
             direct_model={{source:'e3d-model',format:'baked-v2',mesh_id:'{mesh_id}'}};"
        ))
        .await
        .unwrap()
        .check()
        .unwrap();

        let query = "SELECT anc, booled_id != NONE AS has_neg, \
            IF booled_id != NONE { [{geo_hash:booled_id}] } ELSE { \
              (SELECT (transform ?? trans.d) AS transform, record::id(out) AS geo_hash \
               FROM out->geo_relate \
               WHERE visible && out.meshed && (transform ?? trans.d) != NONE && geo_type='Pos') \
            } AS insts \
            FROM type::thing('inst_relate','1_3')";
        let mut response = db.query(query).await.unwrap().check().unwrap();
        let rows = response.take::<Vec<Value>>(0).unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0]["has_neg"], json!(true));
        assert_eq!(rows[0]["insts"][0]["geo_hash"], json!(mesh_id));
    }

    /// 增量那一档先按 id 把整组表示删掉——`INSERT IGNORE` 撞上已有 id 会保留旧行。
    #[test]
    fn a_record_is_deleted_before_it_is_inserted() {
        let sql = model_db_adapter::render_removals(&["1_2".to_string()]);
        for table in ["geo_relate", "inst_info", "inst_relate", "aabb", "trans"] {
            assert!(sql.contains(table), "{table} 没被删到：{sql}");
        }
        assert!(
            !sql.contains("inst_geo"),
            "共享网格按 mesh 做键，谁也不删它"
        );
    }
}
