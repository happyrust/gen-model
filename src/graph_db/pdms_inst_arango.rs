use std::collections::HashMap;
use std::mem::take;
use std::ops::Mul;
use std::sync::Arc;
use std::time::Instant;

use aios_core::pdms_types::{EleGeoInstance, EleGeosInfo, PdmsElement, PdmsMeshInstanceMgr, RefU64};
use anyhow::anyhow;
use arangors_lite::{AqlQuery, Connection, Database};
use bevy::prelude::{dbg, Transform};
use futures::future::ok;
use glam::{Quat, Vec3, Vec4};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use serde_json::value::Value;
use sqlx::Row;

use crate::api::project_mdb::query_mdb_contain_numbdb;
use crate::aql_api::convert_refno_vec_from_vec_string;
use crate::consts::*;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::get_arangodb_conn_from_db_option;
use crate::graph_db::structs::{PdmsEleGraphEdge, PdmsEleGraphNode, PdmsInstanceGraphEdge};
use crate::helper::qualified_table_name;
use crate::options::DbOption;
use aios_core::rvm_types::RvmGeoInfo;

// todo 改成多线程
pub async fn sync_instance_to_graph_db(mgr: Arc<AiosDBManager>, instance_mgr: &PdmsMeshInstanceMgr) -> anyhow::Result<()> {
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
    let pdms_instances = "pdms_instances";
    let aql = AqlQuery::new("
    FOR c IN 0..10 inbound @refno pdms_edges
        PRUNE document(@collection,c._key) != null
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
        .bind_var("refno", refno_aql)
        .bind_var("collection", pdms_instances);
    let result: Vec<EleGeosInfo> = database.aql_query(aql).await?;
    if result.is_empty() { return Ok(None); }
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

pub async fn query_rvm_instance_data_from_refno_aql(refno: RefU64, database: &Database) -> anyhow::Result<Option<RvmGeoInfo>> {
    let refno_aql = refno.to_url_refno();
    let aql = AqlQuery::new("
    let r = document('pdms_instances',@key)
    return {
        '_key':r._key,
        'aabb':r.aabb,
        'world_transform':r.world_transform
    }").bind_var("key", refno_aql);
    let mut result: Vec<RvmGeoInfo> = database.aql_query(aql).await?;
    if result.is_empty() { return Ok(None); }
    Ok(Some(result.remove(0)))
}

#[test]
fn test_get_matrix() {
    let world_transform = bevy::prelude::Transform {
        translation: Vec3::from([2950., 7370., 3105.]),
        rotation: Quat::from_xyzw(0.0, 0.0, 0.0, 1.0),
        scale: Vec3::from([1.0, 1.0, 1.0]),
    };
    let inverse = world_transform.compute_matrix().inverse();
    let min = Vec3::from([2720.0, 7220.0, 2790.0]);
    let max = Vec3::from([3180.0, 7520.0, 3420.0]);
    let min_bbox = inverse.transform_point3(min);
    let max_bbox = inverse.transform_point3(max);
    dbg!(&min_bbox);
    dbg!(&max_bbox);
}