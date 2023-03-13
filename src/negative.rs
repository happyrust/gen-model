use std::collections::HashMap;
use aios_core::negative_mesh_type::{NegativeEdges, NegativeEles};
use aios_core::pdms_types::{GeoHash, PdmsElement, RefU64};
use aios_core::shape::pdms_shape::PdmsMesh;
use arangors_lite::{AqlQuery, Database};
use sqlx::{MySql, Pool};
use crate::aql_api::PdmsElementAql;
use crate::graph_db::pdms_arango::{get_arangodb_conn_from_db_option, save_arangodb_with_database};
use crate::options::DbOption;

async fn boolean_negative_mesh(refno: RefU64, pool: &Pool<MySql>, database: &Database) -> anyhow::Result<(HashMap<RefU64, GeoHash>, HashMap<GeoHash, PdmsMesh>)> {
    let mut negative_eles = HashMap::new();
    let mut negative_edges = HashMap::new();
    let need_compute_refnos = query_negative_refnos_aql(refno, database).await?;
    for (refno, negative_refnos) in need_compute_refnos {
        let (hash, mesh) = compute_boolean_mesh(refno, negative_refnos);
        negative_eles.entry(hash).or_insert(mesh);
        negative_edges.entry(refno).or_insert(hash);
    }
    Ok((negative_edges, negative_eles))
}

pub async fn save_boolean_negative_mesh(refno: RefU64, pool: &Pool<MySql>, database: &Database) -> anyhow::Result<()> {
    let (negative_edges, negative_eles) = boolean_negative_mesh(refno, pool, database).await?;
    let mut eles_vec = Vec::new();
    let eles = negative_eles.into_iter().collect::<Vec<_>>();
    for (hash, mesh) in eles {
        eles_vec.push(NegativeEles {
            _key: hash.to_string(),
            mesh: mesh.into_compress_bytes(),
        })
    }
    if let Ok(eles_json) = serde_json::to_value(&eles_vec) {
        let _ = save_arangodb_with_database(eles_json, "negative_eles", database).await;
    }

    let mut edges_vec = Vec::new();
    let edges_json = negative_edges.into_iter().collect::<Vec<_>>();
    for (refno, hash) in edges_json {
        let key = refno.hash_with_another_refno(RefU64(hash));
        edges_vec.push(NegativeEdges {
            _key: key.to_string(),
            _from: format!("pdms_eles/{}", refno.to_url_refno()),
            _to: format!("negative_eles/{}", hash.to_string()),
        });
    }
    if let Ok(edges_json) = serde_json::to_value(&eles_vec) {
        let _ = save_arangodb_with_database(edges_json, "negative_edges", database).await;
    }
    Ok(())
}

// refno : 基本体的 refno  negative_refnos ： 负实体的集合
fn compute_boolean_mesh(refno: RefU64, negative_refnos: Vec<PdmsElement>) -> (GeoHash, PdmsMesh) {
    todo!()
}

async fn query_negative_refnos_aql(refno: RefU64, database: &Database) -> anyhow::Result<HashMap<RefU64, Vec<PdmsElement>>> {
    let mut map = HashMap::new();
    let key = format!("{}/{}", "pdms_eles", refno.to_url_refno());
    let aql = AqlQuery::new("
    for c in 1..1000 inbound @id pdms_edges
    filter length(
        for z in 1 inbound c._id pdms_edges
            return 1
        ) == 0
    filter c.noun in ['NBOX','NCYL','NPYR']
    return {
        'refno':c._key,
        'owner':c.owner,
        'name':c.name,
        'noun':c.noun,
        'version':0,
        'children_count':0,
    }").bind_var("id", key);
    let result: Vec<PdmsElementAql> = database.aql_query(aql).await?;
    for v in result {
        if let Some(pdms_element) = v.change_to_pdms_element() {
            map.entry(pdms_element.owner).or_insert_with(Vec::new).push(pdms_element);
        }
    }
    Ok(map)
}

#[tokio::test]
async fn test_query_negative_refnos_aql() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option(&db_option).await?;
    let result = query_negative_refnos_aql(RefU64::from_refno_str("23584/6799").unwrap(), &database).await?;
    dbg!(&result);
    Ok(())
}