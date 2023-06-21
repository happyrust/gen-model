use std::collections::{BTreeSet, HashMap};
use std::mem::take;
use std::ops::Mul;
use std::sync::Arc;
use std::time::Instant;
use aios_core::geom_types::RvmGeoInfo;

use aios_core::pdms_types::*;
use anyhow::anyhow;
use bb8_arangodb::arangors_lite::{AqlQuery, Database};
use bevy::prelude::{dbg, Transform};
use futures::future::ok;
use glam::{Mat3, Quat, Vec3, Vec4};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use serde_json::value::Value;
use sqlx::Row;

use crate::api::project_mdb::query_db_nums_of_mdb;
use crate::aql_api::convert_refno_vec_from_vec_string;
use crate::consts::*;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::{ArDatabase, connect_arangodb};
use crate::graph_db::structs::*;
use aios_core::helper::*;
use crate::aql_api::children::{query_deep_children_refnos_fuzzy, query_travel_children_with_types_aql};
use crate::consts::{AQL_PDMS_ELES_COLLECTION};

///保存instance 数据到数据库
pub async fn save_instance_to_graph_db(mgr: &AiosDBManager, inst_mgr: &ShapeInstancesData) -> anyhow::Result<()> {
    let collection = AQL_PDMS_INST_GEO_COLLECTION;
    let edge_collection = "instance_edges";
    let database = mgr.get_arango_db().await?;
    let mut instances = vec![];
    let mut edges = vec![];
    println!("开始保存instance数据");
    for chunk in &inst_mgr.inst_geos_map.iter().chunks(1000) {
        for k in chunk {
            let json = serde_json::to_value(k.1).unwrap();
            instances.push(json);
        }
        let aql = AqlQuery::new(r#"LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection  OPTIONS { ignoreErrors: true , overwriteMode: "replace" }"#)
            .bind_var("@collection", collection)
            .bind_var("elements", take(&mut instances))
            ;
        database.aql_query::<Vec<()>>(aql).await?;
    }

    //保存tubi的数据
    let collection = AQL_PDMS_INST_TUBI_COLLECTION;
    for chunk in &inst_mgr.inst_tubi_map.iter().chunks(1000) {
        for (_, k) in chunk {
            let json = serde_json::to_value(k).unwrap();
            instances.push(json);
        }
        let aql = AqlQuery::new(r#"LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection  OPTIONS { ignoreErrors: true , overwriteMode: "replace" }"#)
            .bind_var("@collection", collection)
            .bind_var("elements", take(&mut instances))
            ;
        database.aql_query::<Vec<()>>(aql).await?;
    }

    // let collection = AQL_PDMS_INST_EDGE_COLLECTION;
    // for chunk in &inst_mgr.geo_edges.iter().chunks(1000) {
    //     for k in chunk {
    //         let json = serde_json::to_value(k).unwrap();
    //         instances.push(json);
    //     }
    //     let aql = AqlQuery::new(r#"LET data = @elements
    //                 FOR d IN data
    //                     INSERT d INTO @@collection  OPTIONS { ignoreErrors: true , overwriteMode: "replace" }"#)
    //         .bind_var("@collection", collection)
    //         .bind_var("elements", take(&mut instances))
    //         ;
    //     database.aql_query::<Vec<()>>(aql).await?;
    // }

    let collection = AQL_PDMS_INST_INFO_COLLECTION;
    for chunk in &inst_mgr.inst_info_map.iter().chunks(1000) {
        for k in chunk {
            let json = serde_json::to_value(k.1).unwrap();
            instances.push(json);
            let edge = PdmsInstanceGraphEdge {
                _from: format!("{AQL_PDMS_ELES_COLLECTION}/{}", k.0.to_refno_normal_string()),
                _to: format!("{}/{}", collection, k.0.to_refno_normal_string()),
            };
            edges.push(serde_json::to_value(&edge).unwrap());
        }
        let aql = AqlQuery::new(r#"LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection  OPTIONS { ignoreErrors: true , overwriteMode: "replace" }"#)
            .bind_var("@collection", collection)
            .bind_var("elements", take(&mut instances))
            ;
        database.aql_query::<Vec<()>>(aql).await?;
        let aql = AqlQuery::new(r#"LET data = @edges
                    FOR d IN data
                        INSERT d INTO @@collection OPTIONS { ignoreErrors: true, overwriteMode: "replace" }"#)
            .bind_var("@collection", edge_collection)
            .bind_var("edges", take(&mut edges))

            ;
        database.aql_query::<Vec<()>>(aql).await?;
    }
    Ok(())
}



///获取element inst的几何数据
pub async fn query_insts_shape_data(database: &ArDatabase, refnos: &[RefU64]) -> anyhow::Result<ShapeInstancesData> {
    let refno_strs = refnos.into_iter().map(|x| x.to_url_refno()).collect::<Vec<_>>();
    // dbg!(&refnos);
    //过滤掉负实体计算后的多余几何体
    let aql = AqlQuery::new(r#"
            FOR refno in @refnos
                FOR c,e,p IN 0..20 inbound CONCAT('pdms_eles/',refno) pdms_edges
                    PRUNE c.noun in @neg_nouns
                    OPTIONS { order: "bfs"  }
                    let parent = p.vertices[-2]
                    let f = document('pdms_inst_infos', c._key)
                    filter f != null and (parent == null or document('pdms_inst_infos', parent._key).geo_type != "Compound")
                    return f
            "#)
        .bind_var("refnos", refno_strs.clone())
        .bind_var("neg_nouns", GENRAL_NEG_NOUN_NAMES.to_vec())
        ;
    let geos_info: Vec<EleGeosInfo> = database.aql_query(aql).await.unwrap();
    let mut inst_info_map = HashMap::new();
    let mut inst_keys = geos_info.iter().map(|x| x.get_inst_key().to_string()).collect::<Vec<_>>();
    for g in geos_info {
        inst_info_map.insert(g.refno, g);
    }

    //还有的直段会放在branch上，需要特殊处理

    inst_keys.push("2".to_string());
    // dbg!(&inst_keys);
    let mut inst_geos_map = HashMap::new();
    // let mut geo_hashes = BTreeSet::new();
    let aql = AqlQuery::new(r#"
            FOR inst_key in @inst_keys
                let f = document('pdms_inst_geos', inst_key)
                filter f != null
                return f
            "#)
        .bind_var("inst_keys", inst_keys)

        ;
    let inst_geos: Vec<EleInstGeosData> = database.aql_query(aql).await.unwrap();
    // dbg!(inst_geos.len());
    for g in inst_geos {
        inst_geos_map.insert(g.inst_key, g);
    }

    let mut inst_tubi_map = HashMap::new();
    let mut all_refnos = inst_info_map.keys().map(|x| x.to_url_refno()).collect::<Vec<_>>();
    //这里需要直接通过这个查询下面的所有的branch那些
    let branch_refnos = query_deep_children_refnos_fuzzy(&database, refnos, &CATA_HAS_TUBI_GEO_NAMES).await?;
    all_refnos.extend(branch_refnos.iter().map(|x| x.to_url_refno()));
    let aql = AqlQuery::new(r#"
            FOR r in @refnos
                let f = document('pdms_inst_tubis', r)
                filter f != null
                return f
            "#)
        .bind_var("refnos", all_refnos)
        ;
    let inst_tubi: Vec<EleGeosInfo> = database.aql_query(aql).await.unwrap();
    for g in inst_tubi {
        inst_tubi_map.insert(g.refno, g);
    }

    return Ok(ShapeInstancesData{
        inst_info_map,
        inst_tubi_map,
        inst_geos_map,
    });
}

pub async fn query_instance_level_with_refno_in_arangodb(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Vec<RefU64>> {
    let refno_aql = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let pdms_inst_infos = AQL_PDMS_INST_INFO_COLLECTION;
    let aql = AqlQuery::new("
    FOR c IN 1..15 inbound @refno pdms_edges
        PRUNE document(@collection,c._key) != null
        Filter document(@collection,c._key) != null
        let f = document(@collection,c._key)
        return f._key")
        .bind_var("refno", refno_aql)
        .bind_var("collection", pdms_inst_infos)
        ;
    let result: Vec<String> = database.aql_query(aql).await.unwrap();
    if result.is_empty() { return Ok(vec![]); }
    let result = convert_refno_vec_from_vec_string(result);
    Ok(result)
}

pub async fn query_instance_level_with_ssc_refno_in_arangodb(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Vec<RefU64>> {
    let refno_aql = format!("ssc_eles/{}", refno.to_url_refno());
    let pdms_inst_infos = AQL_PDMS_INST_INFO_COLLECTION;
    let aql = AqlQuery::new("
    FOR c IN 0..20 inbound @refno ssc_edges
        // Filter document(@collection,c._key) != null
        return c._key")
        .bind_var("refno", refno_aql);
        // .bind_var("collection", pdms_inst_infos);
    let result: Vec<String> = database.aql_query(aql).await?;
    if result.is_empty() { return Ok(vec![]); }
    let result = convert_refno_vec_from_vec_string(result);
    Ok(result)
}

/// 查找基本体得 instance
pub async fn query_rvm_instance_data_from_refno_aql(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Option<RvmGeoInfo>> {
    let refno_aql = refno.to_url_refno();
    let aql = AqlQuery::new("
    let r = document('pdms_inst_infos',@key)
    return {
        '_key':r._key,
        'aabb':r.aabb,
        'geo_insts':r.geo_insts,
        'world_transform':r.world_transform
    }").bind_var("key", refno_aql);
    let result = database.aql_query::<RvmGeoInfo>(aql).await;
    if result.is_err() { return Ok(None); }
    let mut result = result.unwrap();
    if result.is_empty() { return Ok(None); }
    Ok(Some(result.remove(0)))
}

#[test]
fn test_get_matrix() {
    let world_transform = bevy::prelude::Transform {
        translation: Vec3::from([12490., 12280., 2835.0]),
        rotation: Quat::from_array([0., 0.7071067690849304, 0., 0.7071067690849304]),
        scale: Vec3::from([210.0, 210.0, 29.0]),
    };
    let inverse = world_transform.compute_matrix().inverse();
    let min = Vec3::from([-105.0, -105.0, 0.0]);
    let max = Vec3::from([105.0, 105.0, 29.0]);
    let min_bbox = inverse.transform_point3(min);
    let max_bbox = inverse.transform_point3(max);
    let rotation = Mat3::from_quat(world_transform.rotation);

    let x_axis = rotation.x_axis * world_transform.scale.x;
    let y_axis = rotation.y_axis * world_transform.scale.y;
    let z_axis = rotation.z_axis * world_transform.scale.z;

    dbg!(&x_axis.normalize());
    dbg!(&y_axis.normalize());
    dbg!(&z_axis.normalize());

    dbg!(&min_bbox);
    dbg!(&max_bbox);
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