use std::io::Write;
use sqlx::Row;
use serde::{Deserialize, Serialize};
use serde_json::value::Value;
use crate::consts::*;
use arangors_lite::{AqlQuery, Connection};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use aios_core::pdms_types::{PdmsElement, RefU64};
use anyhow::anyhow;
use futures::future::ok;
use crate::api::element::query_mdb_dbnos;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::structs::{PdmsEleGraphEdge, PdmsEleGraphNode};
use crate::helper::qualified_table_name;
use crate::options::DbOption;

pub const URL: &str = "http://localhost:8529";

// #[cfg_attr(not(feature = "blocking"), tokio::main)]
// #[cfg_attr(feature = "blocking", maybe_async::must_be_sync)]
pub async fn sync_graph_db(mgr: Arc<AiosDBManager>, db_option: DbOption) -> anyhow::Result<()> {
    let conn = Connection::establish_jwt(URL, "root", "")
        .await
        .unwrap();

    let database = conn.db("pdms").await.unwrap();
    let mut time = Instant::now();
    let project = &db_option.project_name;
    let mdb = &db_option.mdb_name;
    let mut db_nos = db_option.manual_db_nums.clone().unwrap_or_default();

    let default_conn = AiosDBManager::get_default_conn_str(&db_option);
    let pool = AiosDBManager::get_db_pool(&default_conn, project).await.unwrap();
    let sql = format!("SELECT ID, OWNER, TYPE, NAME, NUMBDB  FROM {PDMS_ELEMENTS_TABLE}");
    let results = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await;
    let collection = "pdms_eles";
    let pdms_edge_collection = "pdms_edges";
    let ssc_edge_collection = "ssc_edges";
    match results {
        Ok(vals) => {
            //需不需要按照db numbder 来分别去生成
            //edge 关系也要生成，pdms的edge关系
            for val_chunk in vals.chunks(1000) {
                let mut eles = vec![];
                let mut edges = vec![];
                for val in val_chunk {
                    let refno = (val.get::<i64, _>("ID") as u64).into();
                    let owner = (val.get::<i64, _>("OWNER") as u64).into();
                    //spref 的关系也要提前同步过来
                    //先只保存基本信息
                    let name = val.get::<String, _>("NAME");
                    let type_name = val.get::<String, _>("TYPE");
                    let dbnum = val.get::<i32, _>("NUMBDB");
                    let refno_str = RefU64(refno).to_refno_normal_string();
                    let owner_str = RefU64(owner).to_refno_normal_string();
                    let element = PdmsEleGraphNode {
                        _key: refno_str.clone(),
                        owner: owner_str.clone(),
                        name,
                        noun: type_name,
                        version: 0,
                        dbnum,
                    };
                    let edge = PdmsEleGraphEdge {
                        _from: format!("{}/{refno_str}", &collection),
                        _to: format!("{}/{owner_str}", &collection),
                    };
                    eles.push(element);
                    edges.push(edge);
                }
                let json = serde_json::to_value(&eles).unwrap();
                let aql = AqlQuery::new("LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection")
                    .bind_var("@collection", collection)
                    .bind_var("elements", json);
                let result: Vec<()> = database.aql_query(aql).await.unwrap();

                let json = serde_json::to_value(&edges).unwrap();
                let aql = AqlQuery::new("LET data = @edges
                    FOR d IN data
                        INSERT d INTO @@collection")
                    .bind_var("@collection", pdms_edge_collection)
                    .bind_var("edges", json);
                let result: Vec<()> = database.aql_query(aql).await.unwrap();
            }

            //db worlds
            // let url = AiosDBManager::get_default_conn_str(&mgr.db_option);
            // let info_pool = AiosDBManager::get_db_pool(
            //     &url, format!("PDMS_INFO_DB_{}", mgr.db_option.project_name.to_uppercase()).as_str()).await?;
            // let pool = AiosDBManager::get_db_pool(&url, project).await?;
            // let mdb_dbnos_map = query_mdb_dbnos(&pool, &info_pool).await?;
            // let key_str = format!("/{mdb}");
            // if mdb_dbnos_map.contains_key(&key_str) {
            //     db_nos = mdb_dbnos_map.get(&key_str).unwrap().get("DESI").cloned().unwrap_or_default();
            // }

            //CACHED_REFNO_BASIC_MAP
        }
        Err(e) => {
            dbg!(&e);
            dbg!(sql);
            return Err(anyhow!(e.to_string()));
        }
    }
    println!("sync graph db costs: {}ms", time.elapsed().as_millis());
    Ok(())
}