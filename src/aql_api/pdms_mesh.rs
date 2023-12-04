use std::collections::{HashMap, HashSet};
use std::process::id;
use aios_core::negative_mesh_type::NegativeEles;
use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use aios_core::shape::pdms_shape::{PlantMesh};
use bb8_arangodb::arangors_lite::{AqlQuery, Database};
use bevy_transform::prelude::Transform;
use dashmap::DashMap;
use glam::{Quat, Vec3};
use serde::{Serialize, Deserialize};
use sqlx::{MySql, Pool, Row};
use crate::consts::{AQL_PDMS_EDGES_COLLECTION, AQL_PDMS_MESH_COLLECTION, PDMS_MESH};
use crate::arangodb::ArDatabase;
use std::str::FromStr;
use crate::consts::AQL_PDMS_ELES_COLLECTION;
use crate::test::common::get_arangodb_conn_from_db_option_for_test;

#[derive(Serialize, Deserialize, Debug, Default)]
struct PdmsMeshAql {
    pub refno: String,
    pub hash: String,
    pub data: String,
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct PdmsMeshWorldTransformAql {
    pub refno: String,
    pub hash: String,
    pub trans: Transform,
    pub data: String,
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct PdmsCatrMeshAql {
    pub refno: u64,
    pub hash: String,
    pub data: String,
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct PdmsMeshQueryData {
    pub hash: String,
    pub data: String,
}


pub async fn query_refno_meshes_aql(refno: RefU64, database: &ArDatabase) -> anyhow::Result<DashMap<RefU64, PlantMesh>> {
    let mut map = DashMap::new();
    let key = format!("{}/{}", AQL_PDMS_ELES_COLLECTION, refno.to_url_refno());
    let aql = AqlQuery::new("\
    With @@pdms_eles,@@pdms_edges
    let refnos = (for v,e,p in 0..10 inbound @id @@pdms_edges
                return v._key)
    let results = ( for refno in refnos
                let r = document('pdms_inst_infos',refno)
                filter r != null
                return { refno:r._key , data:r.data } )
    for result in results
        let refno = result.refno
        for d in result.data
            let r = document('pdms_mesh',d.geo_hash)
            return { refno:refno,hash: r._key, data: r.data }
    ")
        .bind_var("id", key)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    if let Ok(results) = database.aql_query::<PdmsMeshAql>(aql).await {
        if !results.is_empty() {
            for result in results {
                let r = hex::decode(result.data)?;
                if let Ok(mesh) = PlantMesh::from_compress_bytes(&r) {
                    let refno = RefU64::from_str(&result.refno);
                    if refno.is_err() { continue; }
                    let refno = refno.unwrap();
                    map.entry(refno).or_insert(mesh);
                }
            }
        }
    }
    Ok(map)
}

/// 查询多个参考号对应的 mesh
pub async fn query_refnos_meshes_aql(refnos: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<DashMap<RefU64, (Transform, PlantMesh)>> {
    let mut map = DashMap::new();
    let ids = RefU64::to_arangodb_ids(AQL_PDMS_ELES_COLLECTION, refnos);
    let aql = AqlQuery::new("\
    With @@pdms_eles,@@pdms_edges
    for id in @ids
    let refnos = (for v,e,p in 0..10 inbound id @@pdms_edges
                return v._key)
    let results = ( for refno in refnos
                let r = document('pdms_inst_infos',refno)
                filter r != null
                return { refno:r._key , trans: r.world_transform ,data:r.data } )
    for result in results
        let refno = result.refno
        for d in result.data
            let r = document('pdms_mesh',d.geo_hash)
            return { refno:refno,hash: r._key, trans:d.trans,data: r.data }
    ").bind_var("@ids", ids)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    if let Ok(results) = database.aql_query::<PdmsMeshWorldTransformAql>(aql).await {
        for result in results {
            let Ok(refno) = RefU64::from_str(&result.refno) else { continue; };
            let r = hex::decode(result.data)?;
            let Ok(mesh) = PlantMesh::from_compress_bytes(&r) else { continue; };
            map.entry(refno).or_insert((result.trans, mesh));
        }
    }
    Ok(map)
}

pub async fn query_catr_refnos_meshes_aql(refno: RefU64, database: &ArDatabase) -> anyhow::Result<DashMap<RefU64, PlantMesh>> {
    let mut map = DashMap::new();
    let key = format!("{}/{}", AQL_PDMS_ELES_COLLECTION, refno.to_url_refno());
    let aql = AqlQuery::new("\
    With @@pdms_eles,@@pdms_edges
    let refnos = (for v,e,p in 0..10 inbound @id @@pdms_edges
                return v._key)
    let results = ( for refno in refnos
                let r = document('pdms_inst_infos',refno)
                filter r != null
                return { refno:r._key , data:r.data } )
    for result in results
        let refno = result.refno
        for d in result.data
            let r = document('pdms_mesh',d.geo_hash)
            return { refno:d.refno,hash: r._key, data: r.data }
    ")
        .bind_var("id", key)
        .bind_var("@pdms_eles", AQL_PDMS_ELES_COLLECTION)
        .bind_var("@pdms_edges", AQL_PDMS_EDGES_COLLECTION);
    if let Ok(results) = database.aql_query::<PdmsCatrMeshAql>(aql).await {
        if !results.is_empty() {
            for result in results {
                let r = hex::decode(result.data)?;
                if let Ok(mesh) = PlantMesh::from_compress_bytes(&r) {
                    let refno = RefU64(result.refno);
                    map.entry(refno).or_insert(mesh);
                }
            }
        }
    }
    Ok(map)
}

///查询相应的mesh数据
pub async fn query_all_geo_hashs(database: &ArDatabase) -> anyhow::Result<HashSet<u64>> {
    let aql = AqlQuery::new("\
    With @@pdms_mesh
    for d in @@pdms_mesh
        filter d != null
        return d._key
    ").bind_var("@pdms_mesh", AQL_PDMS_MESH_COLLECTION);
    let mut hashs = HashSet::new();
    let results: Vec<String> = database.aql_query(aql).await?;
    for result in results {
        if let Ok(s) = result.parse::<u64>() {
            hashs.insert(s);
        }
    }
    Ok(hashs)
}

///查询相应的mesh数据
pub async fn query_pdms_mesh_aql(database: &ArDatabase, hashes: impl IntoIterator<Item=&u64>) -> anyhow::Result<PlantMeshesData> {
    let mut cache_mgr = PlantMeshesData::default();
    let hash_strs = hashes.into_iter().map(|x| x.to_string()).collect::<Vec<_>>();
    // dbg!(&hash_strs);
    let aql = AqlQuery::new("\
    With @@pdms_mesh
    for hash in @hashes
        let d = document(@@pdms_mesh,hash)
        filter d != null
        return d
    ")
        .bind_var("hashes", hash_strs)
        .bind_var("@pdms_mesh", AQL_PDMS_MESH_COLLECTION);
    let results: Vec<PlantGeoData> = database.aql_query(aql).await?;
    for result in results {
        cache_mgr.meshes.entry(result.geo_hash).or_insert(result);
    }
    Ok(cache_mgr)
}

///查询相应的mesh数据
pub async fn query_pdms_mesh_from_hash_str_aql(database: &ArDatabase, hash_strs: Vec<String>) -> anyhow::Result<PlantMeshesData> {
    let mut cache_mgr = PlantMeshesData::default();
    let aql = AqlQuery::new("\
    With @@pdms_mesh
    for hash in @hashes
        let d = document(@@pdms_mesh,hash)
        filter d != null
        return d
    ")
        .bind_var("hashes", hash_strs)
        .bind_var("@pdms_mesh", AQL_PDMS_MESH_COLLECTION);
    let results: Vec<PlantGeoData> = database.aql_query(aql).await?;
    for result in results {
        cache_mgr.meshes.entry(result.geo_hash).or_insert(result);
    }
    Ok(cache_mgr)
}

//
// pub async fn query_pdms_negative_mesh_from_refno(refno: RefU64, database: &ArDatabase) -> anyhow::Result<PlantMeshesData> {
//     let id = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
//     let aql = AqlQuery::new("
//     for v in 0..5 inbound @id pdms_edges
//         let r = document('negative_eles',v._key)
//         filter r!= null
//         return {
//             '_key': r._key,
//             'mesh': r.mesh
//         }").bind_var("id", id);
//     let results: Vec<NegativeEles> = database.aql_query(aql).await?;
//     let mut cache_mgr = PlantMeshesData::default();
//     for r in results {
//         let refno = RefU64::from_str(&r._key);
//         if refno.is_err() { continue; }
//         let refno = refno.unwrap();
//         let mesh = PlantMesh::from_compress_bytes(&hex::decode(r.mesh)?);
//         if mesh.is_err() { continue; }
//         let mesh = mesh.unwrap();
//         cache_mgr.meshes.entry(refno.0).or_insert(mesh);
//     }
//     Ok(cache_mgr)
// }


pub async fn query_refno_transform(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Option<Transform>> {
    let aql = AqlQuery::new("return document('pdms_inst_infos',@key).world_transform")
        .bind_var("key", refno.to_url_refno());
    let result: Vec<(Quat, Vec3, Vec3)> = database.aql_query(aql).await?;
    if result.is_empty() { return Ok(None); }
    let result = result.first().unwrap();
    let transform = Transform {
        translation: result.1,
        rotation: result.0,
        scale: result.2,
    };
    Ok(Some(transform))
}

fn gen_query_pdms_mesh_from_refno_sql(hash: Vec<u64>) -> String {
    let mut sql = format!("SELECT * FROM {PDMS_MESH} WHERE HASH IN (");
    for h in hash {
        sql.push_str(&format!("{} ,", h));
    }
    sql.remove(sql.len() - 1);
    sql.push_str(")");
    sql
}

#[tokio::test]
async fn test_query_pdms_mesh_aql() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let hashes = vec![546828117367565544, 1418680084324994534];
    let meshes = query_pdms_mesh_aql(&database, hashes.iter()).await?;
    dbg!(&meshes.meshes.len());
    Ok(())
}

#[tokio::test]
async fn test_query_pdms_instance_mesh_from_refno() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let refno = RefU64::from_str("24383/69713").unwrap();
    let result = query_refno_transform(refno, &database).await?;
    Ok(())
}