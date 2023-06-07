use std::collections::{HashMap, HashSet};
use aios_core::negative_mesh_type::NegativeEles;
use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use aios_core::shape::pdms_shape::{PlantMesh};
use bb8_arangodb::arangors::{AqlQuery, Database};
use bevy::prelude::Transform;
use dashmap::DashMap;
use glam::{Quat, Vec3};
use serde::{Serialize, Deserialize};
use sqlx::{MySql, Pool, Row};
use crate::consts::PDMS_MESH;
use crate::graph_db::pdms_arango::ArDatabase;
use crate::graph_db::pdms_inst_arango::*;
use crate::consts::AQL_PDMS_ELES_COLLECTION;
use crate::test::common::get_arangodb_conn_from_db_option;

#[derive(Serialize, Deserialize, Debug, Default)]
struct PdmsMeshAql {
    pub refno: String,
    pub hash: String,
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

pub async fn query_pdms_mesh_data(hash: Vec<u64>, pool: &Pool<MySql>) -> anyhow::Result<MeshesData> {
    let mut cache_mgr = MeshesData::default();
    let query_sql = gen_query_pdms_mesh_from_refno_sql(hash);
    let results = sqlx::query(&query_sql).fetch_all(&mut pool.acquire().await?).await;
    if let Ok(results) = results {
        for result in results {
            let hash = result.get::<u64, _>("HASH");
            let mesh = result.get::<Vec<u8>, _>("MESH");
            let mesh = PlantMesh::from_compress_bytes(&mesh);
            if mesh.is_none() { continue; }
            let mesh = mesh.unwrap();
            cache_mgr.meshes.entry(hash).or_insert(mesh);
        }
    }
    Ok(cache_mgr)
}

pub async fn query_refno_meshes_aql(refno: RefU64, database: &ArDatabase) -> anyhow::Result<DashMap<RefU64, PlantMesh>> {
    let mut map = DashMap::new();
    let key = format!("{}/{}", "pdms_eles", refno.to_url_refno());
    let aql = AqlQuery::builder().query("\
    let refnos = (for v,e,p in 0..10 inbound @id pdms_edges
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
    ").bind_var("id", key).build();
    if let Ok(results) = database.aql_query::<PdmsMeshAql>(aql).await {
        if !results.is_empty() {
            for result in results {
                let r = hex::decode(result.data)?;
                if let Some(mesh) = PlantMesh::from_compress_bytes(&r) {
                    let refno = RefU64::from_url_refno(&result.refno);
                    if refno.is_none() { continue; }
                    let refno = refno.unwrap();
                    map.entry(refno).or_insert(mesh);
                }
            }
        }
    }
    Ok(map)
}

pub async fn query_catr_refnos_meshes_aql(refno: RefU64, database: &ArDatabase) -> anyhow::Result<DashMap<RefU64, PlantMesh>> {
    let mut map = DashMap::new();
    let key = format!("{}/{}", "pdms_eles", refno.to_url_refno());
    let aql = AqlQuery::builder().query("\
    let refnos = (for v,e,p in 0..10 inbound @id pdms_edges
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
    ").bind_var("id", key).build();
    if let Ok(results) = database.aql_query::<PdmsCatrMeshAql>(aql).await {
        if !results.is_empty() {
            for result in results {
                let r = hex::decode(result.data)?;
                if let Some(mesh) = PlantMesh::from_compress_bytes(&r) {
                    let refno = RefU64(result.refno);
                    map.entry(refno).or_insert(mesh);
                }
            }
        }
    }
    Ok(map)
}

///查询相应的mesh数据
pub async fn query_pdms_mesh_aql(database: &ArDatabase, hashes: &[u64]) -> anyhow::Result<MeshesData> {
    let mut cache_mgr = MeshesData::default();
    let hash_strs = hashes.into_iter().map(|x| x.to_string()).collect::<Vec<_>>();
    // dbg!(&hash_strs);
    let aql = AqlQuery::builder().query("\
    for hash in @hashes
        let d = document('pdms_mesh',hash)
        filter d != null
        return {
            'hash':hash,
            'data' : d.data
        }
    ").bind_var("hashes", hash_strs).build();
    let results: Vec<PdmsMeshQueryData> = database.aql_query(aql).await?;
    for result in results {
        let hash: u64 = result.hash.parse()?;
        let data = hex::decode(&result.data)?;
        let mesh = PlantMesh::from_compress_bytes(&data);
        if mesh.is_none() { continue; }
        let mesh = mesh.unwrap();
        cache_mgr.meshes.entry(hash).or_insert(mesh);
    }
    Ok(cache_mgr)
}


pub async fn query_pdms_negative_mesh_from_refno(refno: RefU64, database: &ArDatabase) -> anyhow::Result<MeshesData> {
    let id = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::builder().query("
    for v in 0..5 inbound @id pdms_edges
        let r = document('negative_eles',v._key)
        filter r!= null
        return {
            '_key': r._key,
            'mesh': r.mesh
        }").bind_var("id", id).build();
    let results: Vec<NegativeEles> = database.aql_query(aql).await?;
    let mut cache_mgr = MeshesData::default();
    for r in results {
        let refno = RefU64::from_url_refno(&r._key);
        if refno.is_none() { continue; }
        let refno = refno.unwrap();
        let mesh = PlantMesh::from_compress_bytes(&hex::decode(r.mesh)?);
        if mesh.is_none() { continue; }
        let mesh = mesh.unwrap();
        cache_mgr.meshes.entry(refno.0).or_insert(mesh);
    }
    Ok(cache_mgr)
}


pub async fn query_refno_transform(refno: RefU64, database: &ArDatabase) -> anyhow::Result<Option<Transform>> {
    let aql = AqlQuery::builder().query("return document('pdms_inst_infos',@key).world_transform")
        .bind_var("key", refno.to_url_refno())
        .build();
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
    let database = get_arangodb_conn_from_db_option(&db_option).await?;
    let hashes = vec![546828117367565544, 1418680084324994534];
    let meshes = query_pdms_mesh_aql(&database, &hashes).await?;
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
    let database = get_arangodb_conn_from_db_option(&db_option).await?;
    let refno = RefU64::from_refno_str("24383/69713").unwrap();
    let result = query_refno_transform(refno, &database).await?;
    Ok(())
}