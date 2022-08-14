use aios_core::pdms_types::RefU64;
use arangors_lite::{AqlQuery, Database};
use sqlx::{MySql, Pool, Row};
use crate::graph_db::structs::{PdmsEleGraphEdge, SSCEleGraphNode};

/// 将 ssc固定节点保存到图数据库（zone下面的层级除外）
pub async fn set_arangodb_all_ssc_fixed_nodes(pool: &Pool<MySql>, database: &Database) -> anyhow::Result<()> {
    let sql = gen_query_all_ssc_fixed_nodes_sql();
    let results = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    let collection = "ssc_eles";
    let ssc_edge_collection = "ssc_edges";
    for result_chunk in results.chunks(1000) {
        let mut ssc_eles = vec![];
        let mut ssc_ele_edges = vec![];
        for val in result_chunk {
            let refno = RefU64(val.get::<i64, _>("ID") as u64);
            let owner = RefU64(val.get::<i64, _>("OWNER") as u64);
            let name = val.get::<String, _>("NAME");
            let type_name = val.get::<String, _>("TYPE");
            let refno_str = RefU64::to_refno_normal_string(&refno);
            let owner_str = RefU64::to_refno_normal_string(&owner);
            let ssc_ele = SSCEleGraphNode {
                _key: refno_str.clone(),
                owner: owner_str.clone(),
                name,
                noun: type_name,
                real_pdms_refno: "0/0".to_string(),
            };
            let edge = PdmsEleGraphEdge {
                _from: format!("{}/{refno_str}", &collection),
                _to: format!("{}/{owner_str}", &collection),
            };
            ssc_eles.push(ssc_ele);
            ssc_ele_edges.push(edge);
        }
        let json = serde_json::to_value(&ssc_eles).unwrap();
        let aql = AqlQuery::new("LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection")
            .bind_var("@collection", collection)
            .bind_var("elements", json);
        let _result: Vec<()> = database.aql_query(aql).await.unwrap();

        let json = serde_json::to_value(&ssc_ele_edges).unwrap();
        let aql = AqlQuery::new("LET data = @edges
                    FOR d IN data
                        INSERT d INTO @@collection")
            .bind_var("@collection", ssc_edge_collection)
            .bind_var("edges", json);
        let _result: Vec<()> = database.aql_query(aql).await.unwrap();
    }

    Ok(())
}

fn gen_query_all_ssc_fixed_nodes_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID, OWNER, TYPE, NAME, REAL_PDMS_REFNO FROM PDMS_SSC_ELEMENTS"));
    sql
}