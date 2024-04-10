use crate::arangodb::ArDatabase;
use crate::consts::AQL_PDMS_ELES_COLLECTION;
use crate::consts::*;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::structs::*;
use aios_core::geom_types::RvmGeoInfo;
use aios_core::geometry::{EleGeosInfo, GeoBasicType, ShapeInstancesData};
use aios_core::pdms_types::*;
use aios_core::shape::pdms_shape::RsVec3;
use aios_core::types::*;
use aios_core::SUL_DB;
use bb8_arangodb::arangors_lite::AqlQuery;
use bevy_transform::prelude::Transform;
use dashmap::DashMap;
use glam::{Mat3, Quat, Vec3};
use itertools::Itertools;
use sqlx::Row;
use std::collections::HashMap;
use std::mem::take;

///保存instance 数据到数据库
pub async fn save_instance_data(
    mgr: &AiosDBManager,
    inst_mgr: &ShapeInstancesData,
) -> anyhow::Result<()> {
    println!("开始保存instance数据");

    //保存inst geos 数据
    let keys = inst_mgr.inst_geos_map.keys().collect::<Vec<_>>();
    let mut join_set = tokio::task::JoinSet::new();
    let mut aabb_map: HashMap<u64, String> = HashMap::new();
    let mut transform_map: HashMap<u64, String> = HashMap::new();
    //标识单位矩阵
    transform_map.insert(0, serde_json::to_string(&Transform::IDENTITY).unwrap());
    let mut param_map = HashMap::new();
    let mut vec3_map = HashMap::new();
    let chunk_size = 50;
    for chunk in keys.chunks(chunk_size) {
        let mut json_vec = vec![];
        let mut geo_relate_vec = vec![];
        for &k in chunk {
            let v = inst_mgr.inst_geos_map.get(k).unwrap();
            for inst in &v.insts {
                if inst.transform.is_nan() {
                    dbg!(&inst);
                    continue;
                }
                let transform_hash = gen_bytes_hash::<_, 64>(&inst.transform);
                if !transform_map.contains_key(&transform_hash) {
                    transform_map.insert(
                        transform_hash,
                        serde_json::to_string(&inst.transform).unwrap(),
                    );
                }
                let param_hash = gen_bytes_hash::<_, 64>(&inst.geo_param);
                if !param_map.contains_key(&param_hash) {
                    param_map.insert(param_hash, serde_json::to_string(&inst.geo_param).unwrap());
                }
                let key_pts = inst.geo_param.key_points();
                let mut pt_hashes = vec![];
                for k in key_pts {
                    let pts_hash = k.gen_hash();
                    pt_hashes.push(format!("vec3:⟨{}⟩", pts_hash));
                    if !vec3_map.contains_key(&pts_hash) {
                        vec3_map.insert(pts_hash, serde_json::to_string(&k).unwrap());
                    }
                }
                //还需要加入geo_param的指向，param 是否填原始参数？ param=param:{}
                //使用cata_key -> inst_geos
                let mut relate_sql = format!(
                    "relate inst_info:⟨{}⟩->geo_relate->inst_geo:⟨{}⟩ set trans=trans:⟨{}⟩, geom_refno=pe:{}, param=param:⟨{}⟩, pts=[{}], geo_type='{}', visible={}",
                    v.id(),
                    inst.geo_hash,
                    transform_hash,
                    inst.refno,
                    param_hash,
                    pt_hashes.join(","),
                    inst.geo_type.to_string(),
                    inst.visible
                );
                if !inst.cata_neg_refnos.is_empty() {
                    relate_sql.push_str(&format!(
                        ", cata_neg=[{}]",
                        inst.cata_neg_refnos.iter().map(|x| x.to_pe_key()).join(",")
                    ));
                }
                // dbg!(&relate_sql);
                geo_relate_vec.push(relate_sql);
                //保存 unit shape 的几何参数
                json_vec.push(inst.gen_unit_geo_sur_json());
            }
        }

        if !json_vec.is_empty() {
            let mut sql_string = "".to_string();
            for json in &json_vec {
                sql_string.push_str(&format!("insert ignore into {} {};", stringify!(inst_geo), json));
            }
            //使用surreal 保存NamedAttrMap
            join_set.spawn(async move {
                SUL_DB.query(sql_string).await.unwrap();
            });

            //保存relate 关系
            if !geo_relate_vec.is_empty() {
                //使用surreal 保存NamedAttrMap
                // dbg!(&geo_relate_vec);
                join_set.spawn(async move {
                    SUL_DB.query(geo_relate_vec.join(";")).await.unwrap();
                });
            }
        }
    }
    while let Some(_) = join_set.join_next().await {}

    //保存tubi的数据
    let keys = inst_mgr.inst_tubi_map.keys().collect::<Vec<_>>();
    for chunk in keys.chunks(chunk_size) {
        for &k in chunk {
            let v = inst_mgr.inst_tubi_map.get(k).unwrap();

            //更新aabb 和 transform，保存relate已经在别的地方加了，这里后面需要重构
            let aabb = v.aabb.unwrap();
            let aabb_hash = gen_bytes_hash::<_, 64>(&aabb);
            let tansform_hash = gen_bytes_hash::<_, 64>(&v.world_transform);
            if !aabb_map.contains_key(&aabb_hash) {
                aabb_map.insert(aabb_hash, serde_json::to_string(&aabb).unwrap());
            }
            if !transform_map.contains_key(&tansform_hash) {
                transform_map.insert(
                    tansform_hash,
                    serde_json::to_string(&v.world_transform).unwrap(),
                );
            }
        }
    }

    let keys = inst_mgr.inst_info_map.keys().collect::<Vec<_>>();
    let mut join_set = tokio::task::JoinSet::new();
    for chunk in keys.chunks(chunk_size) {
        let mut json_vec = vec![];
        let mut inst_relate_vec = vec![];
        for &k in chunk {
            let v = inst_mgr.inst_info_map.get(k).unwrap();
            if v.world_transform.is_nan() {
                continue;
            }
            json_vec.push(v.gen_sur_json(&mut vec3_map));

            let transform_hash = gen_bytes_hash::<_, 64>(&v.world_transform);
            if !transform_map.contains_key(&transform_hash) {
                transform_map.insert(
                    transform_hash,
                    serde_json::to_string(&v.world_transform).unwrap(),
                );
            }

            let mut neg_refnos = v.neg_refnos.clone();
            if let Some(refnos) = inst_mgr.ngmr_relate_map.get(k) {
                // dbg!(&refnos);
                neg_refnos.extend(refnos);
            }

            //arrive 和 leave 需要用 index
            //这里的 pts，存储的时点集信息
            // "flow_pt_indexs": self.flow_pt_indexs.clone(),
            let mut sql = format!(
                "relate pe:{k}->inst_relate->inst_info:⟨{}⟩ set world_trans=trans:⟨{}⟩,generic='{}', has_cata_neg={}",
                v.id_str(),
                transform_hash,
                v.generic_type.to_string(),
                v.has_cata_neg,
            );

            if !neg_refnos.is_empty() {
                sql.push_str(&format!(",neg_refnos=[{}]", neg_refnos.iter().map(|x| x.to_pe_key()).join(",")));
                // dbg!(&sql);
            }
            // dbg!(&sql);
            inst_relate_vec.push(sql);
        }

        if !json_vec.is_empty() {
            let mut sql_string = "".to_string();
            for json in &json_vec {
                sql_string.push_str(&format!("insert ignore into {} {};", stringify!(inst_info), json));
            }
            // dbg!(&sql_string);
            //使用surreal 保存NamedAttrMap
            join_set.spawn(async move {
                SUL_DB.query(sql_string).await.unwrap();
            });
        }
        if !inst_relate_vec.is_empty() {
            //使用surreal 保存NamedAttrMap
            // dbg!(&inst_relate_vec);
            join_set.spawn(async move {
                SUL_DB.query(inst_relate_vec.join(";")).await.unwrap();
            });
        }
    }
    while let Some(_) = join_set.join_next().await {}
    // dbg!("insert inst_relate, inst_info ok");

    let mut join_set = tokio::task::JoinSet::new();
    //保存aabb
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
            join_set.spawn(async move {
                SUL_DB.query(sql).await.unwrap();
            });
        }
    }
    //保存transform
    if !transform_map.is_empty() {
        let keys = transform_map.keys().collect::<Vec<_>>();
        for chunk in keys.chunks(100) {
            let mut sql_string = "".to_string();
            for &&k in chunk {
                let v = transform_map.get(&k).unwrap();
                let json = format!("INSERT IGNORE INTO trans {{'id':trans:⟨{}⟩, 'd':{}}};", k, v);
                sql_string.push_str(&json);
            }
            join_set.spawn(async move {
                SUL_DB.query(sql_string).await.unwrap();
            });
        }
    }

    //保存param_map数据
    if !param_map.is_empty() {
        let keys = param_map.keys().collect::<Vec<_>>();
        for chunk in keys.chunks(100) {
            let mut sql_string = "".to_string();
            for &&k in chunk {
                let v = param_map.get(&k).unwrap();
                let json = format!("INSERT IGNORE INTO param {{'id':param:⟨{}⟩, 'd':{}}};", k, v);
                sql_string.push_str(&json);
            }
            join_set.spawn(async move {
                SUL_DB.query(sql_string).await.unwrap();
            });
        }
    }

    if !vec3_map.is_empty() {
        let keys = vec3_map.keys().collect::<Vec<_>>();
        for chunk in keys.chunks(100) {
            let mut sql_string = "".to_string();
            for &&k in chunk {
                let v = vec3_map.get(&k).unwrap();
                let json = format!("INSERT IGNORE INTO vec3 {{'id':vec3:⟨{}⟩, 'd':{}}};", k, v);
                sql_string.push_str(&json);
            }
            join_set.spawn(async move {
                SUL_DB.query(sql_string).await.unwrap();
            });
        }
    }

    while let Some(_) = join_set.join_next().await {}

    // dbg!("insert vec3, trans, param ok");

    Ok(())
}

///获取element inst的几何数据
/// 默认直接优先取负实体的数据
pub async fn query_insts_shape_data(
    database: &ArDatabase,
    refnos: impl IntoIterator<Item = &RefU64>,
    geo_type_filter: Option<&[GeoBasicType]>,
) -> anyhow::Result<ShapeInstancesData> {
    Ok(Default::default())
}

pub async fn query_instance_level_with_refno_in_arangodb(
    refno: RefU64,
    database: &ArDatabase,
) -> anyhow::Result<Vec<RefU64>> {
    Ok(Vec::new())
}

pub async fn query_instance_level_with_ssc_refno_in_arangodb(
    refno: RefU64,
    database: &ArDatabase,
) -> anyhow::Result<Vec<RefU64>> {
    return Ok(vec![]);
}

/// 查找基本体得 instance
pub async fn query_rvm_instance_data_from_refno_aql(
    refno: RefU64,
    database: &ArDatabase,
) -> anyhow::Result<Option<RvmGeoInfo>> {
    let refno_aql = refno.to_string();
    let aql = AqlQuery::new(
        "
    With @@pdms_inst_infos
    let r = document('pdms_inst_infos',@key)
    return {
        '_key':r._key,
        'aabb':r.aabb,
        'geo_insts':r.geo_insts,
        'world_transform':r.world_transform
    }",
    )
    .bind_var("key", refno_aql)
    .bind_var("@pdms_inst_infos", AQL_PDMS_INST_INFO_COLLECTION);
    let result = database.aql_query::<RvmGeoInfo>(aql).await;
    if result.is_err() {
        return Ok(None);
    }
    let mut result = result.unwrap();
    if result.is_empty() {
        return Ok(None);
    }
    Ok(Some(result.remove(0)))
}

pub async fn query_rvm_instance_data_from_owner_aql(
    refno: RefU64,
    database: &ArDatabase,
) -> anyhow::Result<Vec<RvmGeoInfo>> {
    let refno_aql = format!("{}/{}", AQL_PDMS_ELES_COLLECTION, refno.to_string());
    let aql = AqlQuery::new(
        "
    With @@pdms_inst_infos,@@pdms_eles,@@pdms_edges
    for v in 1 inbound @key @@pdms_edges
    let r = document(@@pdms_inst_infos,v._key)
    filter r != null
    return {
        '_key':r._key,
        'aabb':r.aabb,
        'data':[],
        'world_transform':r.world_transform
    }",
    )
    .bind_var("key", refno_aql)
    .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
    .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION)
    .bind_var("@pdms_inst_infos", AQL_PDMS_INST_INFO_COLLECTION);
    let result = database.aql_query::<RvmGeoInfo>(aql).await?;
    Ok(result)
}

pub async fn query_compound_inst_hashes_aql(
    refnos: Vec<RefU64>,
    database: &ArDatabase,
) -> anyhow::Result<Vec<EleGeosInfo>> {
    let ids = refnos
        .into_iter()
        .map(|x| x.to_string())
        .collect::<Vec<_>>();
    let aql = AqlQuery::new(
        "\
    With @@pdms_compound_inst_infos
    for id in @ids
    let compound_inst = document(@@pdms_compound_inst_infos,id)
    filter compound_inst != null
    return compound_inst
    ",
    )
    .bind_var("ids", ids)
    .bind_var(
        "@pdms_compound_inst_infos",
        AQL_PDMS_COMPOUND_INST_INFO_COLLECTION,
    );
    let result = database.aql_query::<EleGeosInfo>(aql).await?;
    Ok(result)
}

#[test]
fn test_get_matrix() {
    // let world_transform = bevy::prelude::Transform {
    //     translation: Vec3::from([12490., 12280., 2835.0]),
    //     rotation: Quat::from_array([0., 0.7071067690849304, 0., 0.7071067690849304]),
    //     scale: Vec3::from([210.0, 210.0, 29.0]),
    // };
    // let inverse = world_transform.compute_matrix().inverse();
    // let min = Vec3::from([-105.0, -105.0, 0.0]);
    // let max = Vec3::from([105.0, 105.0, 29.0]);
    // let min_bbox = inverse.transform_point3(min);
    // let max_bbox = inverse.transform_point3(max);
    // let rotation = Mat3::from_quat(world_transform.rotation);
    //
    // let x_axis = rotation.x_axis * world_transform.scale.x;
    // let y_axis = rotation.y_axis * world_transform.scale.y;
    // let z_axis = rotation.z_axis * world_transform.scale.z;
    //
    // dbg!(&x_axis.normalize());
    // dbg!(&y_axis.normalize());
    // dbg!(&z_axis.normalize());
    //
    // dbg!(&min_bbox);
    // dbg!(&max_bbox);
}

#[test]
fn test_cata_transform() {
    let desi_transform = Transform {
        translation: Vec3::from([5360.43994140625, 16279.5, 2596.780029296875]),
        rotation: Quat::from_array([0.0, 0.0, 0.7071067690849304, -0.7071067690849304]),
        scale: Vec3::from([1., 1., 1.]),
    };

    let cata_transform = Transform {
        translation: Vec3::from([0.0, 0.0, 0.0]),
        rotation: Quat::from_array([0.0, 0.7071067690849304, 0.0, 0.7071067690849304]),
        scale: Vec3::from([210.0, 210.0, 29.0]),
    };

    let total_transform = cata_transform * desi_transform;

    let rotation = Mat3::from_quat(total_transform.rotation);
    let scale = total_transform.scale;
    let x_axis = rotation.x_axis * scale.x;
    let y_axis = rotation.y_axis * scale.y;
    let z_axis = rotation.z_axis * scale.z;
    dbg!(total_transform.translation);
    dbg!(x_axis.normalize());
    dbg!(y_axis.normalize());
    dbg!(z_axis.normalize());
}
