use aios_core::pdms_types::{CachedMeshesMgr, GeoHash, RefU64};
use aios_core::shape::pdms_shape::{PdmsInstanceMeshMap, PdmsMesh};
use arangors_lite::{AqlQuery, Database};
use dashmap::DashMap;
use serde::{Serialize, Deserialize};
use sqlx::{MySql, Pool, Row};
use crate::consts::PDMS_MESH;

#[derive(Serialize, Deserialize, Debug, Default)]
struct PdmsMeshAql {
    pub refno: String,
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
        let refno = RefU64::from_url_refno(result.refno);
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

fn gen_query_pdms_mesh_from_refno_sql(hash: Vec<u64>) -> String {
    let mut sql = format!("SELECT * FROM {PDMS_MESH} WHERE HASH IN (");
    for h in hash {
        sql.push_str(&format!("{} ,", h));
    }
    sql.remove(sql.len() - 1);
    sql.push_str(")");
    sql
}