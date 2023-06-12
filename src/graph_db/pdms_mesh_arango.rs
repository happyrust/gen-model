use std::sync::Arc;
use std::io::Write;
use aios_core::pdms_types::MeshesData;
use bb8_arangodb::arangors::collection::CollectionType::Document;
use itertools::Itertools;
use log::{error, info};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::{connect_arangodb, create_arango_document, save_arangodb_doc};
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshMgrArangodb {
    pub _key: String,
    pub data: String,
}

pub async fn save_mesh_to_arango_db(mgr: &AiosDBManager, mesh_mgr: &MeshesData) -> anyhow::Result<()> {
    let mut result = vec![];
    for (&hash, mesh) in &mesh_mgr.meshes {
        // 将 mesh 转换成二进制并压缩
        let mesh_bin = hex::encode(mesh.into_compress_bytes());
        result.push(MeshMgrArangodb {
            _key: hash.to_string(),
            data: mesh_bin,
        })
    }
    // let database = mgr.get_arangodb_conn().await?;
    let database = mgr.get_arango_db().await?;
    create_arango_document(&database, "pdms_mesh", Document).await?;
    let json = serde_json::to_value(&result)?;
    println!("开始保存mesh数据");
    save_arangodb_doc(json, "pdms_mesh", &database, mgr.db_option.replace_dbs).await?;
    Ok(())
}