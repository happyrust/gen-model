use std::collections::HashMap;
use std::mem::take;
use std::sync::Arc;
use std::time::Instant;

use aios_core::pdms_types::{PdmsElement, PdmsMeshInstanceMgr, RefU64};
use anyhow::anyhow;
use arangors_lite::{AqlQuery, Connection};
use futures::future::ok;
use serde::{Deserialize, Serialize};
use serde_json::value::Value;
use sqlx::Row;

use crate::api::element::query_mdb_dbnos;
use crate::api::project_mdb::query_mdb_contain_numbdb;
use crate::consts::*;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::structs::{PdmsEleGraphEdge, PdmsEleGraphNode, PdmsInstanceGraphEdge};
use crate::helper::qualified_table_name;
use crate::options::DbOption;

// todo 改成多线程
pub async fn sync_instance_to_graph_db(mgr: Arc<AiosDBManager>, instance_mgr: Arc<PdmsMeshInstanceMgr>) -> anyhow::Result<()> {
    let mut time = Instant::now();
    let collection = "pdms_instances";
    let edge_collection = "instance_edges";

    let database = mgr.arango_database.clone();
    let mut instances = vec![];
    let mut edges = vec![];
    for k in &instance_mgr.inst_mgr.inst_map {
        let json = serde_json::to_string(k.value()).unwrap();
        instances.push(json);
        let edge = PdmsInstanceGraphEdge {
            _from: format!("pdms_eles/{}", k.key().to_refno_normal_string()),
            _to: format!("{}/{}", collection, k.key().to_refno_normal_string()),
        };
        edges.push(serde_json::to_string(&edge).unwrap());
        if instances.len() == 1000 {
            let aql = AqlQuery::new("LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection")
                .bind_var("@collection", collection)
                .bind_var("elements", take(&mut instances));
            database.aql_query::<Vec<()>>(aql).await?;

            let aql = AqlQuery::new("LET data = @edges
                    FOR d IN data
                        INSERT d INTO @@collection")
                .bind_var("@collection", edge_collection)
                .bind_var("edges", take(&mut edges));
            database.aql_query::<Vec<()>>(aql).await?;
        }
    }


//     let edge = PdmsEleGraphEdge {
//         _from: format!("{}/{refno_str}", &collection),
//         _to: format!("{}/{owner_str}", &collection),
//     };
//     eles.push(element);
//     edges.push(edge);
// }
// let json = serde_json::to_value(&eles).unwrap();
// let aql = AqlQuery::new("LET data = @elements
//                     FOR d IN data
//                         INSERT d INTO @@collection")
// .bind_var("@collection", collection)
// .bind_var("elements", json);
// let result: Vec<()> = database.aql_query(aql).await.unwrap();
//
// let json = serde_json::to_value(&edges).unwrap();
// let aql = AqlQuery::new("LET data = @edges
//                     FOR d IN data
//                         INSERT d INTO @@collection")
// .bind_var("@collection", pdms_edge_collection)
// .bind_var("edges", json);
// let result: Vec<()> = database.aql_query(aql).await.unwrap();


    Ok(())
}