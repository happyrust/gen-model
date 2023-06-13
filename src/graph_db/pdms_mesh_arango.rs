use std::sync::Arc;
use std::io::Write;
use std::mem::take;
use aios_core::pdms_types::PlantMeshesData;
use arangors::AqlQuery;
use bb8_arangodb::arangors::collection::CollectionType::Document;
use itertools::Itertools;
use log::{error, info};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::{connect_arangodb, create_arango_document, save_arangodb_doc};
use serde::{Serialize, Deserialize};
use crate::consts::AQL_PDMS_MESH_COLLECTION;

pub async fn save_mesh_to_arango_db(mgr: &AiosDBManager, mesh_mgr: &PlantMeshesData) -> anyhow::Result<()> {
    let collection = AQL_PDMS_MESH_COLLECTION;
    let database = mgr.get_arango_db().await?;
    let mut data = vec![];
    println!("开始保存instance数据");
    for chunk in &mesh_mgr.meshes.iter().chunks(1000) {
        for k in chunk {
            let json = serde_json::to_value(k.1).unwrap();
            data.push(json);
        }
        let aql = AqlQuery::builder().query(r#"LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection  OPTIONS { ignoreErrors: true , overwriteMode: "replace" }"#)
            .bind_var("@collection", collection)
            .bind_var("elements", take(&mut data))
            .build();
        database.aql_query::<Vec<()>>(aql).await.unwrap();
    }

    Ok(())
}