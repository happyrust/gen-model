use crate::data_interface;
use crate::fast_model::gen_all_geos_data;
use aios_core::options::DbOption;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::prim_geo::basic::OccSharedShape;
use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::test::test_surreal::init_test_surreal;
use aios_core::{gen_bytes_hash, RefU64, SUL_DB};
use bevy_transform::prelude::Transform;
use itertools::Itertools;
use opencascade::primitives::{Compound, IntoShape, Shape};
use parry3d::bounding_volume::*;
use parry3d::math::Isometry;
use std::collections::HashMap;
use std::path::PathBuf;

///生成小的几何体
#[tokio::test]
pub async fn test_gen_geos() -> anyhow::Result<()> {
    init_test_surreal().await;
    //首先查询 inst_geo
    process_gen_meshes(Some(&["17496/171559".into()]))
        .await
        .unwrap();
    Ok(())
}

pub async fn process_gen_meshes(refnos: Option<&[RefU64]>) -> anyhow::Result<()> {
    //首先查询 inst_geo
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()
        .unwrap();
    let mut db_option = s.try_deserialize::<DbOption>().unwrap();
    if let Some(refnos) = refnos {
        db_option.debug_root_refnos =
            Some(refnos.iter().map(|x| x.to_string()).collect::<Vec<_>>());
    }
    let mgr = data_interface::tidb_manager::AiosDBManager::init(&db_option)
        .await
        .unwrap();
    println!("正在生成模型");
    let time = std::time::Instant::now();
    gen_all_geos_data(std::sync::Arc::new(mgr), None)
        .await
        .unwrap();
    println!("生成模型花费时间: {} ms", time.elapsed().as_millis());

    let time = std::time::Instant::now();
    gen_inst_meshes(None).await.unwrap();
    update_inst_relate_aabbs().await.unwrap();
    apply_insts_boolean(None).await.unwrap();
    println!(
        "更新数据库和布尔运算花费时间: {} ms",
        time.elapsed().as_millis()
    );
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct QueryGeoParam {
    pub id: String,
    pub param: PdmsGeoParam,
}

pub async fn gen_inst_meshes(dir: Option<PathBuf>) -> anyhow::Result<()> {
    //首先查询 inst_geo
    //需要扫描当前的文件夹，是否有已经生成的几何体
    //使用page 去扫描？
    let dir = dir.unwrap_or("assets/meshes".into());
    //如果dir 不存在，创建这个目录
    if !dir.exists() {
        std::fs::create_dir_all(&dir).unwrap();
    }

    const PAGE_NUM: usize = 100;
    let mut i = 0;
    let mut shapes_map: HashMap<String, (OccSharedShape, f64)> = HashMap::new();
    loop {
        let mut response = SUL_DB.query(&format!("select meta::id(id) as id, param from inst_geo where !meshed start {} limit {PAGE_NUM}", i * PAGE_NUM)).await?;
        let result: Vec<QueryGeoParam> = response.take(0).unwrap();
        if result.is_empty() {
            break;
        }
        i += 1;
        // dbg!(&result);
        for g in result {
            //如果属于 负实体关联的几何体，需要提前保存到hashmap，然后单独生成
            if let Some(shape) = g.param.gen_occ_shape() {
                let mut aabb = Aabb::new_invalid();
                for edge in shape.edges() {
                    for point in edge.approximation_segments() {
                        aabb.take_point(nalgebra::Point3::new(
                            point.x as f32,
                            point.y as f32,
                            point.z as f32,
                        ));
                    }
                }
                shapes_map.insert(g.id, (shape, aabb.half_extents().magnitude() as f64 * 0.01));
            }
        }
    }

    let mut update_sql = vec![];
    let mut aabb_map: HashMap<u64, String> = HashMap::new();
    for (id, (s, tol)) in shapes_map {
        // dbg!(tol);
        if let Ok(mesh) = PlantMesh::gen_occ_mesh(&s, tol) {
            //保存到文件到dir下
            if mesh.ser_to_file(&dir.join(format!("{}.mesh", id))).is_ok() {
                let aabb_hash = gen_bytes_hash::<_, 64>(&mesh.aabb);
                update_sql.push(format!(
                    "update inst_geo:⟨{}⟩ set meshed = true, aabb = aabb:⟨{}⟩;",
                    id, aabb_hash
                ));
                aabb_map
                    .entry(aabb_hash)
                    .or_insert(serde_json::to_string(&mesh.aabb).unwrap());
            }
        }
    }
    if !update_sql.is_empty() {
        //执行SUL_DB update,使用chunk 保存
        for update in update_sql.chunks(100) {
            SUL_DB.query(update.into_iter().join("\n")).await.unwrap();
        }

        //更新aabb数据到数据库
        save_aabb_to_surreal(&aabb_map).await?;
    }

    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct QueryAabbParam {
    pub id: RefU64,
    pub geo_aabbs: Vec<GeoAabbTrans>,
    pub world_trans: Transform,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct GeoAabbTrans {
    pub trans: Transform,
    pub aabb: Aabb,
}

///刷新inst_relate 的 aabb
pub async fn update_inst_relate_aabbs() -> anyhow::Result<()> {
    //todo 使用分页来实现刷新

    let sql = r#"select in as id, world_trans.d as world_trans,
            (select out.aabb.d as aabb, trans.d as trans from out->geo_relate) as geo_aabbs from inst_relate where aabb == none"#;

    let mut response = SUL_DB.query(sql).await?;
    let result: Vec<QueryAabbParam> = response.take(0).unwrap();
    // dbg!(&result);

    let mut aabb_map: HashMap<u64, String> = HashMap::new();
    for r in result {
        let mut aabb = Aabb::new_invalid();
        for g in r.geo_aabbs {
            let t = r.world_trans * g.trans;
            let tmp_aabb = g.aabb.scaled(&t.scale.into());
            let tmp_aabb = tmp_aabb.transform_by(&Isometry {
                rotation: t.rotation.into(),
                translation: t.translation.into(),
            });
            aabb.merge(&tmp_aabb);
        }
        let aabb_hash = gen_bytes_hash::<_, 64>(&aabb);
        aabb_map
            .entry(aabb_hash)
            .or_insert(serde_json::to_string(&aabb).unwrap());
        let sql = format!(
            "update inst_relate set aabb = aabb:⟨{}⟩ where in=pe:{};",
            aabb_hash,
            r.id.to_string()
        );
        // dbg!(&sql);
        SUL_DB.query(&sql).await.unwrap();
    }

    save_aabb_to_surreal(&aabb_map).await?;

    Ok(())
}

async fn save_aabb_to_surreal(aabb_map: &HashMap<u64, String>) -> anyhow::Result<()> {
    if !aabb_map.is_empty() {
        let keys = aabb_map.keys().collect::<Vec<_>>();
        for chunk in keys.chunks(100) {
            let mut jsons = vec![];
            for &&k in chunk {
                let v = aabb_map.get(&k).unwrap();
                let json = format!("{{'id':aabb:⟨{}⟩, 'd':{}}}", k, v);
                jsons.push(json);
            }
            let sql = format!("INSERT IGNORE INTO aabb [{}]", jsons.join(","));
            SUL_DB.query(sql).await?;
        }
    }

    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct GeoParam {
    pub id: String,
    pub param: PdmsGeoParam,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct GeoTransQuery {
    pub refno: RefU64,
    pub wt: Transform,
    pub aabb: Aabb,
    pub ts: Vec<(String, Transform)>,
    //world transform, vec![(id, transform)]
    pub neg_ts: Vec<(Transform, Vec<(String, Transform)>)>,
}

///执行bool 运算
pub async fn apply_insts_boolean(dir: Option<PathBuf>) -> anyhow::Result<()> {
    let dir = dir.unwrap_or("assets/meshes".into());
    //如果dir 不存在，创建这个目录
    if !dir.exists() {
        std::fs::create_dir_all(&dir).unwrap();
    }

    let sql = r#"
        select meta::id(id) as id, param from
         array::group(select value array::group([array::group(neg_refnos->inst_relate->inst_info->geo_relate->inst_geo),
         ->inst_info->geo_relate->inst_geo]) from inst_relate where neg_refnos!=none);
    "#;
    let mut response = SUL_DB.query(sql).await?;
    let params: Vec<GeoParam> = response.take(0).unwrap();
    // dbg!(&params);
    let mut shapes_map: HashMap<String, OccSharedShape> = HashMap::new();
    //todo 根据这些param，生成occ shape
    //然后执行bool 运算
    for g in params {
        //如果属于 负实体关联的几何体，需要提前保存到hashmap，然后单独生成
        let tol = g.param.tol();
        if let Some(shape) = g.param.gen_occ_shape() {
            shapes_map.insert(g.id, shape);
        }
    }

    let sql = r#"
        select
             in as refno,
             world_trans.d as wt,
             aabb.d as aabb,
            (select value [meta::id(out), trans.d] from out->geo_relate) as ts,
            (select value [(world_trans.d)[0], (select value [meta::id(array::first(out)), (trans.d)[0]] from out->geo_relate)]
        from neg_refnos->inst_relate) as neg_ts from inst_relate where neg_refnos!=none
    "#;
    let mut response = SUL_DB.query(sql).await?;
    let boolean_query: Vec<GeoTransQuery> = response.take(0).unwrap();
    // dbg!(&boolean_query);

    for mut b in boolean_query {
        let Some((pos_id, pos_t)) = b.ts.pop() else {
            continue;
        };
        let Some(pos_shape) = shapes_map.get(&pos_id) else {
            continue;
        };
        let inverse_mat = b.wt.compute_matrix().as_dmat4().inverse();
        let pos_matrix = pos_t.compute_matrix().as_dmat4();
        let mut pos_shape = pos_shape.transformed(&pos_matrix);
        // pos_shape.write_step(format!("{}.step", "pos")).unwrap();
        // let mut shapes = vec![pos_shape.clone()];
        let mut final_shape: Option<Shape> = None;
        for n in b.neg_ts {
            let mut neg_shapes = vec![];
            for (neg_id, neg_t) in n.1 {
                if let Some(neg_shape) = shapes_map.get(&neg_id) {
                    let m = inverse_mat * n.0.compute_matrix().as_dmat4() * neg_t.compute_matrix().as_dmat4();
                    // dbg!(m);
                    neg_shapes.push(neg_shape.transformed(&m));
                }
            }
            if !neg_shapes.is_empty() {
                for neg_shape in neg_shapes {
                    // neg_shape.write_step(format!("{}.step", "neg")).unwrap();
                    if let Some(f) = &final_shape {
                        final_shape = Some(f.subtract(&neg_shape).into_shape());
                    }else{
                        final_shape = Some(pos_shape.subtract(&neg_shape).into_shape());
                    }
                }
            }
        }
        if let Some(f) = &final_shape {

            // f.write_step(format!("{}.step", b.refno)).unwrap();

            // let compound = Compound::from_shapes(shapes).into_shape();
            // compound.write_step(format!("{}.step", "compound")).unwrap();

            let tol = b.aabb.half_extents().magnitude() * 0.01;
            // f.write_stl_with_tolerance("test.stl", tol as _).unwrap();
            dbg!(tol);
            dbg!(b.refno);
            if let Ok(mesh) = PlantMesh::gen_occ_mesh(f, tol as _) {
                //保存到文件到dir下
                if mesh
                    .ser_to_file(&dir.join(format!("{}.mesh", b.refno)))
                    .is_ok()
                {
                    // dbg!(tol);
                    // let aabb_hash = gen_bytes_hash::<_, 64>(&mesh.aabb);
                    //如果使用了bool 运算，直接查询参考号对应的几何体就行
                    //todo 是否要更新aabb ?
                    // aabb_map.entry(aabb_hash).or_insert(serde_json::to_string(&mesh.aabb).unwrap());
                }
            }
        }

    }

    Ok(())
}
