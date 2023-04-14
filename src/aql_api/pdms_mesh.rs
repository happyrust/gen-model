use std::collections::HashSet;
use aios_core::negative_mesh_type::NegativeEles;
use aios_core::pdms_data::PdmsInstanceMeshData;
use aios_core::pdms_types::{CachedMeshesMgr, EleGeosInfo, GeoHash, RefU64, ShapeInstancesMgr};
use aios_core::shape::pdms_shape::{PdmsInstanceMeshMap, PdmsMesh};
use arangors_lite::{AqlQuery, Database};
use bevy::prelude::Transform;
use dashmap::DashMap;
use glam::{Quat, Vec3};
use serde::{Serialize, Deserialize};
use sqlx::{MySql, Pool, Row};
use crate::consts::PDMS_MESH;
use crate::graph_db::pdms_arango::get_arangodb_conn_from_db_option;
use crate::graph_db::pdms_inst_arango::*;
use crate::negative::query_instance_refnos_negative_aql;
use crate::options::DbOption;
use crate::AQL_PDMS_ELES_COLLECTION;

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
struct PdmsMeshWithoutRefnoAql {
    pub hash: String,
    pub data: String,
}

pub async fn query_pdms_mesh_from_refno(hash: Vec<u64>, pool: &Pool<MySql>) -> anyhow::Result<CachedMeshesMgr> {
    let mut cache_mgr = CachedMeshesMgr::default();
    let query_sql = gen_query_pdms_mesh_from_refno_sql(hash);
    let results = sqlx::query(&query_sql).fetch_all(&mut pool.acquire().await?).await;
    if let Ok(results) = results {
        for result in results {
            let hash = result.get::<u64, _>("HASH");
            let mesh = result.get::<Vec<u8>, _>("MESH");
            let mesh = PdmsMesh::from_compress_bytes(&mesh);
            if mesh.is_none() { continue; }
            let mesh = mesh.unwrap();
            cache_mgr.meshes.entry(hash).or_insert(mesh);
        }
    }
    Ok(cache_mgr)
}

pub async fn query_refnos_meshes_aql(refno: RefU64, database: &Database) -> anyhow::Result<DashMap<RefU64, PdmsMesh>> {
    let mut map = DashMap::new();
    let key = format!("{}/{}", "pdms_eles", refno.to_url_refno());
    let aql = AqlQuery::new("\
    let refnos = (for v,e,p in 0..10 inbound @id pdms_edges
                return v._key)
    let results = ( for refno in refnos
                let r = document('pdms_instances',refno)
                filter r != null
                return { refno:r._key , data:r.data } )
    for result in results
        let refno = result.refno
        for d in result.data
            let r = document('pdms_mesh',d.geo_hash)
            return { refno:refno,hash: r._key, data: r.data }
    ").bind_var("id", key);
    if let Ok(results) = database.aql_query::<PdmsMeshAql>(aql).await {
        if !results.is_empty() {
            for result in results {
                let r = hex::decode(result.data)?;
                if let Some(mesh) = PdmsMesh::from_compress_bytes(&r) {
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

pub async fn query_catr_refnos_meshes_aql(refno: RefU64, database: &Database) -> anyhow::Result<DashMap<RefU64, PdmsMesh>> {
    let mut map = DashMap::new();
    let key = format!("{}/{}", "pdms_eles", refno.to_url_refno());
    let aql = AqlQuery::new("\
    let refnos = (for v,e,p in 0..10 inbound @id pdms_edges
                return v._key)
    let results = ( for refno in refnos
                let r = document('pdms_instances',refno)
                filter r != null
                return { refno:r._key , data:r.data } )
    for result in results
        let refno = result.refno
        for d in result.data
            let r = document('pdms_mesh',d.geo_hash)
            return { refno:d.refno,hash: r._key, data: r.data }
    ").bind_var("id", key);
    if let Ok(results) = database.aql_query::<PdmsCatrMeshAql>(aql).await {
        if !results.is_empty() {
            for result in results {
                let r = hex::decode(result.data)?;
                if let Some(mesh) = PdmsMesh::from_compress_bytes(&r) {
                    let refno = RefU64(result.refno);
                    map.entry(refno).or_insert(mesh);
                }
            }
        }
    }
    Ok(map)
}

pub async fn query_pdms_mesh_aql(hashes: Vec<u64>, database: &Database) -> anyhow::Result<CachedMeshesMgr> {
    let mut cache_mgr = CachedMeshesMgr::default();
    let hashes = hashes.into_iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let aql = AqlQuery::new("\
    for hash in @hashes
        return {
            'hash':hash,
            'data' : document('pdms_mesh',hash).data
        }
    ").bind_var("hashes", hashes);
    let results: Vec<PdmsMeshWithoutRefnoAql> = database.aql_query(aql).await?;
    for result in results {
        let hash: u64 = result.hash.parse()?;
        let data = hex::decode(&result.data)?;
        let mesh = PdmsMesh::from_compress_bytes(&data);
        if mesh.is_none() { continue; }
        let mesh = mesh.unwrap();
        cache_mgr.meshes.entry(hash).or_insert(mesh);
    }
    Ok(cache_mgr)
}

pub async fn query_pdms_mesh_from_refno_aql(refno: RefU64, database: &Database) -> anyhow::Result<PdmsInstanceMeshMap> {
    let mut mgr = DashMap::new();
    let mut instances = DashMap::new();
    let key = format!("{}/{}", "pdms_eles", refno.to_url_refno());
    let aql = AqlQuery::new("\
    let refnos = (for v,e,p in 0..10 inbound @id pdms_edges
                return v._key)
    let results = ( for refno in refnos
                let r = document('pdms_instances',refno)
                filter r != null
                return { refno:r._key , data:r.data } )
    for result in results
        let refno = result.refno
        for d in result.data
            let r = document('pdms_mesh',d.geo_hash)
            return { refno:refno,hash: r._key, data: r.data }
    ").bind_var("id", key);
    let results: Vec<PdmsMeshAql> = database.aql_query(aql).await?;
    for result in results {
        let hash = result.hash.parse().unwrap_or(0);
        let refno = RefU64::from_url_refno(&result.refno);
        if refno.is_none() { continue; }
        let refno = refno.unwrap();
        instances.entry(refno).or_insert_with(Vec::new).push(hash);

        let r = hex::decode(result.data)?;
        let data = PdmsMesh::from_compress_bytes(&r);
        if data.is_none() { continue; }
        let data = data.unwrap();
        mgr.entry(hash).or_insert(data);
    }
    Ok(PdmsInstanceMeshMap {
        refno_map: instances,
        mesh_map: mgr,
    })
}

pub async fn query_pdms_negative_mesh_from_refno(refno: RefU64, database: &Database) -> anyhow::Result<CachedMeshesMgr> {
    let id = format!("{AQL_PDMS_ELES_COLLECTION}/{}", refno.to_url_refno());
    let aql = AqlQuery::new("
    for v in 0..5 inbound @id pdms_edges
        let r = document('negative_eles',v._key)
        filter r!= null
        return {
            '_key': r._key,
            'mesh': r.mesh
        }").bind_var("id", id);
    let results: Vec<NegativeEles> = database.aql_query(aql).await?;
    let mut cache_mgr = CachedMeshesMgr::default();
    for r in results {
        let refno = RefU64::from_url_refno(&r._key);
        if refno.is_none() { continue; }
        let refno = refno.unwrap();

        let mesh = PdmsMesh::from_compress_bytes(&hex::decode(r.mesh)?);
        if mesh.is_none() { continue; }
        let mesh = mesh.unwrap();
        cache_mgr.meshes.entry(refno.0).or_insert(mesh);
    }
    Ok(cache_mgr)
}

/// 通过参考号获取参考后下面所有的instance 和 对应的 mesh
pub async fn query_pdms_instance_mesh_from_refno(refno: RefU64, database: &Database) -> anyhow::Result<PdmsInstanceMeshData> {
    let mut inst_mgr = ShapeInstancesMgr::default();
    let mut hashes = HashSet::new();
    if let Some(instance) = query_instance_with_refno_in_arangodb(refno, database).await? {
        for inst in instance {
            let refno = RefU64::from_url_refno(&inst._key);
            if refno.is_none() { continue; }
            // 找到参考号需要那些mesh,避免重复
            for data in &inst.data {
                hashes.insert(data.geo_hash);
            }
            let refno = refno.unwrap();
            inst_mgr.inst_map.entry(refno).or_insert(inst);
        }
    }
    let hashes = hashes.into_iter().collect::<Vec<_>>();
    let mesh_mgr = query_pdms_mesh_aql(hashes, database).await.unwrap_or_default();
    Ok(PdmsInstanceMeshData {
        inst_mgr,
        mesh_mgr,
    })
}

pub async fn query_pdms_instance_mesh_from_refnos(refnos: Vec<RefU64>, database: &Database) -> anyhow::Result<PdmsInstanceMeshData> {
    let mut inst_mgr = ShapeInstancesMgr::default();
    let mut hashes = HashSet::new();
    dbg!(&refnos);
    if let Some(instance) = query_instance_with_refnos_in_arangodb(refnos, database).await? {
        for inst in instance {
            let refno = RefU64::from_url_refno(&inst._key);
            if refno.is_none() { continue; }
            // 找到参考号需要那些mesh,避免重复
            for data in &inst.data {
                hashes.insert(data.geo_hash);
            }
            let refno = refno.unwrap();
            inst_mgr.inst_map.entry(refno).or_insert(inst);
        }
    }
    let hashes = hashes.into_iter().collect::<Vec<_>>();
    let mesh_mgr = query_pdms_mesh_aql(hashes, database).await.unwrap_or_default();
    Ok(PdmsInstanceMeshData {
        inst_mgr,
        mesh_mgr,
    })
}

pub async fn query_refno_transform(refno: RefU64, database: &Database) -> anyhow::Result<Option<Transform>> {
    let aql = AqlQuery::new("return document('pdms_instances',@key).world_transform")
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
    let database = get_arangodb_conn_from_db_option(&db_option).await?;
    let hashes = vec![546828117367565544, 1418680084324994534];
    let meshes = query_pdms_mesh_aql(hashes, &database).await?;
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
    // let result = query_pdms_instance_mesh_from_refno(refno,&database).await?;
    let result = query_refno_transform(refno, &database).await?;
    // for data in result.inst_mgr.inst_map {
    //     dbg!(&data.0);
    // }
    // dbg!(&result.mesh_mgr.len());
    dbg!(&result);
    Ok(())
}