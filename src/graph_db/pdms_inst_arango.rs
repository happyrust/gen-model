use std::collections::HashMap;
use std::mem::take;
use std::ops::Mul;
use std::sync::Arc;
use std::time::Instant;
use aios_core::geom_types::RvmGeoInfo;

use aios_core::pdms_types::*;
use anyhow::anyhow;
use arangors_lite::{AqlQuery, Connection, Database};
use bevy::prelude::{dbg, Transform};
use futures::future::ok;
use glam::{Mat3, Quat, Vec3, Vec4};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use serde_json::value::Value;
use sqlx::Row;

use crate::api::project_mdb::query_mdb_contain_numbdb;
use crate::aql_api::convert_refno_vec_from_vec_string;
use crate::consts::*;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::get_arangodb_conn_from_db_option;
use crate::graph_db::structs::*;
use crate::helper::qualified_table_name;
use crate::options::DbOption;

// todo 改成多线程
pub async fn sync_instance_to_graph_db(mgr: Arc<AiosDBManager>, instance_mgr: &CachedInstanceMgr) -> anyhow::Result<()> {
    let collection = "pdms_instances";
    let edge_collection = "instance_edges";

    let database = &get_arangodb_conn_from_db_option(&mgr.db_option).await?;
    let mut instances = vec![];
    let mut edges = vec![];
    for chunk in &instance_mgr.inst_mgr.inst_map.clone().into_iter().chunks(1000) {
        for k in chunk {
            let json = serde_json::to_value(k.1.to_json_type()).unwrap();
            instances.push(json);
            let edge = PdmsInstanceGraphEdge {
                _from: format!("pdms_eles/{}", k.0.to_refno_normal_string()),
                _to: format!("{}/{}", collection, k.0.to_refno_normal_string()),
            };
            edges.push(serde_json::to_value(&edge).unwrap());
        }
        let aql = AqlQuery::new("LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection  OPTIONS { ignoreErrors: true }")
            .bind_var("@collection", collection)
            .bind_var("elements", take(&mut instances));
        database.aql_query::<Vec<()>>(aql).await?;

        let aql = AqlQuery::new("LET data = @edges
                    FOR d IN data
                        INSERT d INTO @@collection OPTIONS { ignoreErrors: true }")
            .bind_var("@collection", edge_collection)
            .bind_var("edges", take(&mut edges));
        database.aql_query::<Vec<()>>(aql).await?;
    }
    Ok(())
}

/// 传入参考号，返回该参考号下面的模型数据
pub async fn query_instance_with_refno_in_arangodb(refno: RefU64, database: &Database) -> anyhow::Result<Option<Vec<EleGeosInfo>>> {
    let refno_aql = format!("pdms_eles/{}", refno.to_url_refno());
    let aql = AqlQuery::new("
    FOR c IN 0..10 inbound @refno pdms_edges
        PRUNE document(@collection,c._key) != null
        Filter document(@collection,c._key) != null
        let f = document(@collection,c._key)
        let p = document(@params_collection, c._key)
        return {
            '_key':f._key,
            'data':f.data,
            'params': p.geo_params,
            'visible':f.visible,
            'generic_type':f.generic_type,
            'aabb':f.aabb,
            'world_transform':f.world_transform,
            'ptset_map':f.ptset_map,
            'flow_pt_indexs':f.flow_pt_indexs
        }")
        .bind_var("refno", refno_aql)
        .bind_var("collection", "pdms_instances")
        .bind_var("params_collection", "geo_infos");
    let result: Vec<EleGeosInfoJson> = database.aql_query(aql).await?;
    if result.is_empty() { return Ok(None); }
    let result = result.into_iter().map(|x| EleGeosInfo::from_json_type(x)).collect::<Vec<_>>();
    Ok(Some(result))
}

pub async fn query_instance_with_refnos_in_arangodb(refno: Vec<RefU64>, database: &Database) -> anyhow::Result<Option<Vec<EleGeosInfo>>> {
    let refnos = refno.into_iter().map(|x| x.to_url_refno()).collect::<Vec<_>>();
    let aql = AqlQuery::new("
    FOR refno in @refnos
        FOR c IN 0..10 inbound CONCAT('pdms_eles/',refno) pdms_edges
            Filter document(@collection,c._key) != null
            let f = document(@collection,c._key)
            return {
                '_key':f._key,
                'data':f.data,
                'visible':f.visible,
                'generic_type':f.generic_type,
                'aabb':f.aabb,
                'world_transform':f.world_transform,
                'ptset_map':f.ptset_map,
                'flow_pt_indexs':f.flow_pt_indexs
            }")
        .bind_var("refnos", refnos)
        .bind_var("collection", "pdms_instances")
        ;
    // dbg!(&aql);
    let result: Vec<EleGeosInfoJson> = database.aql_query(aql).await?;
    if result.is_empty() { return Ok(None); }
    let result = result.into_iter().map(|x| EleGeosInfo::from_json_type(x)).collect::<Vec<_>>();
    Ok(Some(result))
}

pub async fn query_instance_level_with_refno_in_arangodb(refno: RefU64, database: &Database) -> anyhow::Result<Vec<RefU64>> {
    let refno_aql = format!("pdms_eles/{}", refno.to_url_refno());
    let pdms_instances = "pdms_instances";
    let aql = AqlQuery::new("
    FOR c IN 1..15 inbound @refno pdms_edges
        PRUNE document(@collection,c._key) != null
        Filter document(@collection,c._key) != null
        let f = document(@collection,c._key)
        return f._key")
        .bind_var("refno", refno_aql)
        .bind_var("collection", pdms_instances);
    let result: Vec<String> = database.aql_query(aql).await.unwrap();
    if result.is_empty() { return Ok(vec![]); }
    let result = convert_refno_vec_from_vec_string(result);
    Ok(result)
}

pub async fn query_instance_level_with_ssc_refno_in_arangodb(refno: RefU64, database: &Database) -> anyhow::Result<Vec<RefU64>> {
    let refno_aql = format!("ssc_eles/{}", refno.to_url_refno());
    let pdms_instances = "pdms_instances";
    let aql = AqlQuery::new("
    FOR c IN 1..20 inbound @refno ssc_edges
        PRUNE document(@collection,c._key) != null
        Filter document(@collection,c._key) != null
        let f = document(@collection,c._key)
        return f._key")
        .bind_var("refno", refno_aql)
        .bind_var("collection", pdms_instances);
    let result: Vec<String> = database.aql_query(aql).await?;
    if result.is_empty() { return Ok(vec![]); }
    let result = convert_refno_vec_from_vec_string(result);
    Ok(result)
}

/// 查找基本体得 instance
pub async fn query_rvm_instance_data_from_refno_aql(refno: RefU64, database: &Database) -> anyhow::Result<Option<RvmGeoInfo>> {
    let refno_aql = refno.to_url_refno();
    let aql = AqlQuery::new("
    let r = document('pdms_instances',@key)
    return {
        '_key':r._key,
        'aabb':r.aabb,
        'data':r.data,
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