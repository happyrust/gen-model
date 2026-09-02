//! 把 `e3d-model` 直读生成结果落成 Plant UI 现有的 `.mesh + inst_relate`。
//!
//! 这不经 OBJ：同一个 Manifold 实体先变回元素局部系，再走 gen-model
//! 已有的 `manifold_to_plant_mesh` 序列化口径。库侧仍是 Plant UI 已经消费的
//! `inst_relate.booled_id + aabb + world_trans`。未参与布尔的 BOX/CYLI 则复用
//! `inst_geo + geo_relate.trans`，保持旧版 Plant UI 的实例查询协议。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aios_core::shape::pdms_shape::PlantMesh;
use anyhow::{Context, bail};
use e3d_io::db_element::{DbFilePin, DbSet, template_file_for};
use e3d_io::engine::{ReadOnlyEngine, ScanTier};
use e3d_io::refno::RefNo;
use e3d_model::db_discovery::DirectoryDbResolver;
use e3d_model::elmodl::GeometryId;
use e3d_model::pipeline::{Report, generate_subtree};
use e3d_model::primitive_instance::{PrimitiveMeshKey, canonical_primitive_mesh};
use e3d_model::transform::dmat4_to_affine4x3;
use glam::DMat4;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

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

fn update_mesh_digest(hash: &mut Sha256, mesh: &PlantMesh) {
    hash.update((mesh.vertices.len() as u64).to_le_bytes());
    for vertex in &mesh.vertices {
        for value in vertex.to_array() {
            hash.update(value.to_le_bytes());
        }
    }
    hash.update((mesh.normals.len() as u64).to_le_bytes());
    for normal in &mesh.normals {
        for value in normal.to_array() {
            hash.update(value.to_le_bytes());
        }
    }
    hash.update((mesh.indices.len() as u64).to_le_bytes());
    for index in &mesh.indices {
        hash.update(index.to_le_bytes());
    }
}

/// 旧格式的 inst_geo id 是 mesh 文件名，必须只由规范网格身份决定。
fn shared_mesh_identity(key: PrimitiveMeshKey, mesh: &PlantMesh) -> String {
    let mut hash = Sha256::new();
    hash.update(b"e3d-model/primitive-inst-geo/v1\0");
    hash.update(serde_json::to_vec(&key).expect("PrimitiveMeshKey JSON"));
    update_mesh_digest(&mut hash, mesh);
    let bytes: [u8; 8] = hash.finalize()[..8].try_into().expect("sha256 prefix");
    u64::from_be_bytes(bytes).max(1).to_string()
}

fn record_raw_id(refno: RefNo) -> String {
    format!("{}_{}", refno.word0, refno.word1)
}

fn geometry_record_raw_id(geometry_id: &GeometryId, source_refno: RefNo) -> String {
    aios_database::fast_model::e3d_mesh_store::geometry_record_id(geometry_id, source_refno)
}

fn sql_value(value: &Value) -> String {
    serde_json::to_string(value).expect("JSON value")
}

fn old_or_none(row: &Value, field: &str) -> String {
    match row.get(field) {
        None | Some(Value::Null) => "NONE".to_string(),
        Some(Value::String(value))
            if field == "aabb" || field == "world_trans" || field == "out" =>
        {
            value.clone()
        }
        Some(value) => sql_value(value),
    }
}

fn transform_value(matrix: DMat4) -> Value {
    let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
    json!({
        "translation": [translation.x as f32, translation.y as f32, translation.z as f32],
        "rotation": [rotation.x as f32, rotation.y as f32, rotation.z as f32, rotation.w as f32],
        "scale": [scale.x as f32, scale.y as f32, scale.z as f32]
    })
}

/// 内容寻址 mesh 的跨进程写入：胜者原子发布，失败方校验胜者后清理临时文件。
/// 返回 true 表示本调用写入了新文件，false 表示复用了完整一致的现有文件。
fn write_and_verify_mesh(path: &Path, mesh: &PlantMesh) -> anyhow::Result<bool> {
    Ok(matches!(
        aios_database::fast_model::e3d_mesh_store::ensure_mesh_file(path, mesh)?,
        aios_database::fast_model::e3d_mesh_store::MeshWrite::Written
    ))
}

struct SharedUpdateSpec<'a> {
    raw_id: &'a str,
    source_raw_id: &'a str,
    geometry_id: &'a GeometryId,
    noun: &'a str,
    mesh_id: &'a str,
    dbno: u32,
    world_aabb_id: &'a str,
    world_transform_id: &'a str,
    shared_aabb_id: &'a str,
    local_transform_id: &'a str,
    local_bounds: [[f32; 3]; 2],
    local_transform: &'a Value,
    primitive_key: PrimitiveMeshKey,
    anc: &'a [i64],
}

fn render_shared_update(spec: &SharedUpdateSpec<'_>) -> anyhow::Result<String> {
    let SharedUpdateSpec {
        raw_id,
        source_raw_id,
        geometry_id,
        noun,
        mesh_id,
        dbno,
        world_aabb_id,
        world_transform_id,
        shared_aabb_id,
        local_transform_id,
        local_bounds,
        local_transform,
        primitive_key,
        anc,
    } = spec;
    let key_json = sql_value(&serde_json::to_value(primitive_key)?);
    let geometry_id_json = sql_value(&serde_json::to_value(geometry_id)?);
    Ok(format!(
        "UPSERT type::thing('aabb','{shared_aabb_id}') CONTENT {{d:{{mins:{},maxs:{}}}}};\n\
         UPSERT type::thing('trans','{local_transform_id}') CONTENT {{d:{}}};\n\
         UPSERT inst_geo:⟨{mesh_id}⟩ CONTENT {{meshed:true,visible:true,bad:false,aabb:type::thing('aabb','{shared_aabb_id}'),param:{{primitive_key:{key_json}}},direct_model:{{source:'e3d-model',format:'canonical-primitive-v1'}}}};\n\
         UPSERT type::thing('inst_info','direct_{raw_id}') CONTENT {{dbnum:{dbno},noun:'{noun}',direct_model:{{source:'e3d-model',format:'legacy-instanced-v2'}}}};\n\
         DELETE geo_relate:[type::thing('inst_info','direct_{raw_id}'),inst_geo:⟨{mesh_id}⟩];\n\
         INSERT RELATION INTO geo_relate [{{id:geo_relate:[type::thing('inst_info','direct_{raw_id}'),inst_geo:⟨{mesh_id}⟩],in:type::thing('inst_info','direct_{raw_id}'),out:inst_geo:⟨{mesh_id}⟩,geom_refno:type::thing('pe','{raw_id}'),pts:[],geo_type:'Pos',trans:type::thing('trans','{local_transform_id}'),visible:true}}];\n\
         UPSERT type::thing('inst_relate','{raw_id}') SET in=type::thing('pe','{source_raw_id}'), out=type::thing('inst_info','direct_{raw_id}'), booled_id=NONE, booled=false, bad_bool=false, solid=true, generic='{noun}', dbnum={dbno}, anc={}, aabb=type::thing('aabb','{world_aabb_id}'), world_trans=type::thing('trans','{world_transform_id}'), insts_flat=[{{geo_hash:'{mesh_id}',transform:{}}}], direct_model={{source:'e3d-model',format:'canonical-primitive-v1',geometry_id:{geometry_id_json},mesh_id:'{mesh_id}',primitive_key:{key_json}}};\n",
        sql_value(&json!(local_bounds[0])),
        sql_value(&json!(local_bounds[1])),
        sql_value(local_transform),
        sql_value(&json!(anc)),
        sql_value(local_transform),
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

    aios_core::init_surreal()
        .await
        .context("连接兼容目标 SurrealDB")?;
    let db = aios_core::SUL_DB.clone();
    let mut backups = BTreeMap::<String, Value>::new();
    for chunk in generated.chunks(250) {
        let keys = chunk
            .iter()
            .map(|element| {
                format!(
                    "type::thing('inst_relate', '{}')",
                    geometry_record_raw_id(&element.geometry_id, element.refno)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let sql = backup_query(&keys);
        let mut response = db.query(sql).await?.check()?;
        for row in response.take::<Vec<Value>>(0)? {
            let raw = row
                .get("raw_id")
                .and_then(Value::as_str)
                .context("inst_relate backup 缺 raw_id")?;
            backups.insert(raw.to_string(), row);
        }
    }
    let backup_path = args.evidence_dir.join("inst-relation-backup.json");
    std::fs::write(&backup_path, serde_json::to_vec_pretty(&backups)?)?;

    let mut persisted = Vec::with_capacity(generated.len());
    let mut written_shared_meshes = BTreeSet::new();
    let mut mesh_written = 0usize;
    let mut mesh_reused = 0usize;
    let mut update_sql = String::new();
    let mut rollback_sql = String::new();
    for element in generated {
        let raw_id = geometry_record_raw_id(&element.geometry_id, element.refno);
        let source_raw_id = record_raw_id(element.refno);
        let geometry_id_json = sql_value(&serde_json::to_value(&element.geometry_id)?);
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
        let world_aabb_id = format!("direct_world_{raw_id}");
        let world_transform_id = format!("direct_world_{raw_id}");
        let world_transform = transform_value(element.world);

        let (artifact, storage, primitive_key, local_transform_id) = if let Some(instance) =
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
                Some((
                    format!("direct_local_{raw_id}"),
                    transform_value(instance.local_transform),
                )),
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
        update_sql.push_str(&format!(
            "UPSERT type::thing('aabb','{world_aabb_id}') CONTENT {{d:{{mins:{},maxs:{}}}}};\n\
             UPSERT type::thing('trans','{world_transform_id}') CONTENT {{d:{}}};\n\
             UPDATE type::thing('inst_relate','{raw_id}') SET aabb_d={{mins:{},maxs:{}}}, world_trans_d={};\n",
            sql_value(&json!(world_bounds[0])),
            sql_value(&json!(world_bounds[1])),
            sql_value(&world_transform),
            sql_value(&json!(world_bounds[0])),
            sql_value(&json!(world_bounds[1])),
            sql_value(&world_transform),
        ));
        if let Some((local_transform_id, local_transform)) = local_transform_id.as_ref() {
            let shared_aabb_id = format!("direct_shared_{}", artifact.id);
            update_sql.push_str(&render_shared_update(&SharedUpdateSpec {
                raw_id: &raw_id,
                source_raw_id: &source_raw_id,
                geometry_id: &element.geometry_id,
                noun: &element.noun,
                mesh_id: &artifact.id,
                dbno,
                world_aabb_id: &world_aabb_id,
                world_transform_id: &world_transform_id,
                shared_aabb_id: &shared_aabb_id,
                local_transform_id,
                local_bounds,
                local_transform,
                primitive_key: primitive_key.expect("shared key"),
                anc: &anc,
            })?);
        } else {
            update_sql.push_str(&format!(
                "UPSERT type::thing('inst_relate','{raw_id}') SET in=type::thing('pe','{source_raw_id}'), booled_id='{}', booled=true, bad_bool=false, solid=true, generic='{}', dbnum={dbno}, anc={}, aabb=type::thing('aabb','{world_aabb_id}'), world_trans=type::thing('trans','{world_transform_id}'), insts_flat=[{{geo_hash:'{}'}}], direct_model={{source:'e3d-model',format:'baked-v2',geometry_id:{geometry_id_json},mesh_id:'{}'}};\n",
                artifact.id, element.noun, sql_value(&json!(anc)), artifact.id, artifact.id,
            ));
        }
        if let Some(old) = backups.get(&raw_id) {
            rollback_sql.push_str(&format!(
                "UPDATE type::thing('inst_relate','{raw_id}') SET out={}, booled_id={}, booled={}, bad_bool={}, solid={}, generic={}, dbnum={}, anc={}, aabb={}, aabb_d={}, world_trans={}, world_trans_d={}, insts_flat={}, direct_model={};\n",
                old_or_none(old, "out"), old_or_none(old, "booled_id"),
                old_or_none(old, "booled"), old_or_none(old, "bad_bool"),
                old_or_none(old, "solid"), old_or_none(old, "generic"),
                old_or_none(old, "dbnum"), old_or_none(old, "anc"),
                old_or_none(old, "aabb"), old_or_none(old, "aabb_d"),
                old_or_none(old, "world_trans"), old_or_none(old, "world_trans_d"),
                old_or_none(old, "insts_flat"), old_or_none(old, "direct_model"),
            ));
        } else {
            rollback_sql.push_str(&format!("DELETE type::thing('inst_relate','{raw_id}');\n"));
        }
        if local_transform_id.is_some() {
            let inst_info_id = format!("direct_{raw_id}");
            rollback_sql.push_str(&format!(
                "DELETE geo_relate:[type::thing('inst_info','{inst_info_id}'),inst_geo:⟨{}⟩];\nDELETE type::thing('inst_info','{inst_info_id}');\nDELETE type::thing('trans','direct_local_{raw_id}');\n",
                artifact.id,
            ));
        }
        rollback_sql.push_str(&format!(
            "DELETE type::thing('aabb','{world_aabb_id}'); DELETE type::thing('trans','{world_transform_id}');\n"
        ));
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
    std::fs::write(args.evidence_dir.join("update.sql"), &update_sql)?;
    std::fs::write(args.evidence_dir.join("rollback.sql"), &rollback_sql)?;
    std::fs::write(
        args.evidence_dir.join("persisted-elements.json"),
        serde_json::to_vec_pretty(&persisted)?,
    )?;

    if args.persist {
        for statement_chunk in update_sql.lines().collect::<Vec<_>>().chunks(300) {
            db.query(statement_chunk.join("\n")).await?.check()?;
        }
    }
    println!(
        "DIRECT_PERSIST generated={} unique_meshes={} shared_instances={} baked_instances={} mesh_reused={} mesh_written={} db_records={} mode={}",
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
        backups.len(),
        if args.persist { "persist" } else { "dry-run" }
    );
    println!("BACKUP {}", backup_path.display());
    Ok(())
}

fn backup_query(keys: &str) -> String {
    format!(
        "SELECT <string>record::id(id) AS raw_id, booled_id, booled, bad_bool, solid, generic, dbnum, anc, \
         IF aabb = NONE THEN NONE ELSE <string>aabb END AS aabb, aabb_d, \
         IF world_trans = NONE THEN NONE ELSE <string>world_trans END AS world_trans, world_trans_d, \
         IF out = NONE THEN NONE ELSE <string>out END AS out, insts_flat, direct_model FROM [{keys}];"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(geometry_record_raw_id(&tube_a, source).starts_with("derived_"));
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

    #[tokio::test]
    async fn legacy_plant_ui_query_resolves_shared_mesh_and_local_transform() {
        let mesh_id = shared_mesh_identity(PrimitiveMeshKey::BoxV1, &box_mesh());
        let local_transform = json!({
            "translation": [0.0, 0.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": [12.0, 34.0, 56.0]
        });
        let shared_sql = render_shared_update(&SharedUpdateSpec {
            raw_id: "1_2",
            source_raw_id: "1_2",
            geometry_id: &GeometryId::Element {
                refno: "1/2".to_string(),
            },
            noun: "BOX",
            mesh_id: &mesh_id,
            dbno: 1112,
            world_aabb_id: "direct_world_1_2",
            world_transform_id: "direct_world_1_2",
            shared_aabb_id: &format!("direct_shared_{mesh_id}"),
            local_transform_id: "direct_local_1_2",
            local_bounds: [[-0.5, -0.5, 0.0], [0.5, 0.5, 1.0]],
            local_transform: &local_transform,
            primitive_key: PrimitiveMeshKey::BoxV1,
            anc: &[4294967298, 4294967297],
        })
        .unwrap();
        assert!(shared_sql.contains("booled_id=NONE"));
        assert!(shared_sql.contains("INSERT RELATION INTO geo_relate"));
        assert!(shared_sql.contains("anc=["));

        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem db boots");
        db.use_ns("direct_model_test")
            .use_db("legacy_ui")
            .await
            .unwrap();
        let seed = format!(
            "CREATE type::thing('pe','1_2') SET noun='BOX';\n\
             CREATE type::thing('inst_info','old_1_2');\n\
             CREATE type::thing('inst_relate','1_2') SET in=type::thing('pe','1_2'),out=type::thing('inst_info','old_1_2');\n\
             UPSERT type::thing('aabb','direct_world_1_2') CONTENT {{d:{{mins:[0.0,0.0,0.0],maxs:[12.0,34.0,56.0]}}}};\n\
             UPSERT type::thing('trans','direct_world_1_2') CONTENT {{d:{{translation:[1.0,2.0,3.0],rotation:[0.0,0.0,0.0,1.0],scale:[1.0,1.0,1.0]}}}};\n\
             {shared_sql}"
        );
        db.query(seed).await.unwrap().check().unwrap();

        // Keep this selection aligned with old-aios-core::rs_surreal::inst,
        // which is the Plant UI contract for the non-booled branch.
        let query = "SELECT anc, booled_id != NONE AS has_neg, \
            IF booled_id != NONE { [{geo_hash:booled_id}] } ELSE { \
              (SELECT trans.d AS transform, record::id(out) AS geo_hash \
               FROM out->geo_relate \
               WHERE visible && out.meshed && trans.d != NONE && geo_type='Pos') \
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
              (SELECT trans.d AS transform, record::id(out) AS geo_hash \
               FROM out->geo_relate \
               WHERE visible && out.meshed && trans.d != NONE && geo_type='Pos') \
            } AS insts \
            FROM type::thing('inst_relate','1_3')";
        let mut response = db.query(query).await.unwrap().check().unwrap();
        let rows = response.take::<Vec<Value>>(0).unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0]["has_neg"], json!(true));
        assert_eq!(rows[0]["insts"][0]["geo_hash"], json!(mesh_id));
    }

    #[test]
    fn backup_query_preserves_missing_record_links_as_none() {
        let sql = backup_query("type::thing('inst_relate','1_2')");
        assert!(sql.contains("IF aabb = NONE THEN NONE ELSE <string>aabb END"));
        assert!(sql.contains("IF world_trans = NONE THEN NONE ELSE <string>world_trans END"));
        assert!(sql.contains("IF out = NONE THEN NONE ELSE <string>out END"));
    }
}
