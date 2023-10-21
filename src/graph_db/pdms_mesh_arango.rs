use std::sync::Arc;
use std::io::Write;
use std::mem::take;
use aios_core::cache::mgr::BytesTrait;
use aios_core::pdms_types::*;
use arangors_lite::AqlQuery;
use bb8_arangodb::arangors_lite::collection::CollectionType::Document;
use itertools::Itertools;
use log::{error, info};
use nom::AsBytes;
use parry3d::bounding_volume::Aabb;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::{connect_arangodb, create_arango_document, save_arangodb_doc};
use serde::{Serialize, Deserialize};
use crate::aql_api::pdms_mesh::query_all_geo_hashs;
use crate::consts::AQL_PDMS_MESH_COLLECTION;
use crate::data_interface::interface::PdmsDataInterface;


///保存mesh数据到本地缓存
pub fn save_mesh_to_local_db(mgr: &AiosDBManager, mesh_mgr: &PlantMeshesData, replace_mesh: bool) -> anyhow::Result<bool> {
    let mesh_tree = mgr.local_mesh_db.clone();
    let aabb_tree = mgr.local_mesh_aabb_db.clone();
    for (h, p) in &mesh_mgr.meshes {
        if let Some(m) = &p.mesh && let Some(a) = &p.aabb {
            if !replace_mesh {
                //跳过不必要的保存
                if mesh_tree.get(h.to_be_bytes().as_slice())?.is_some() {
                    continue;
                }
            }
            mesh_tree.insert(h.to_be_bytes().as_slice(), m.into_compress_bytes())?;
            aabb_tree.insert(h.to_be_bytes().as_slice(), a.to_bytes()?)?;
        }
    }
    Ok(true)
}

///保存mesh数据到图数据库
pub async fn save_mesh_to_arango_db(mgr: &AiosDBManager, mesh_mgr: &mut PlantMeshesData, replace: bool) -> anyhow::Result<()> {
    let collection = AQL_PDMS_MESH_COLLECTION;
    let database = mgr.get_arango_db().await?;
    let mut data = vec![];
    println!("开始保存mesh数据");

    //不是replace，需要考虑缓存
    let mut meshes = &mut mesh_mgr.meshes;
    println!("当前mesh数量：{}", meshes.len());
    if !replace {
        let exist_geo_hashs = query_all_geo_hashs(&database).await?;
        // dbg!(&exist_geo_hashs);
        println!("数据库已经存在的mesh数量：{}", exist_geo_hashs.len());

        //保存local mesh db 里的数据
        let aabb_tree = mgr.local_mesh_aabb_db.clone();
        // dbg!(aabb_tree.len());
        println!("当前local mesh 数量：{}", aabb_tree.len());

        println!("当前mesh 数量：{}", meshes.len());
        //遍历一遍local db，检查是否有遗漏
        let mut iter = aabb_tree.iter();
        while let Some(Ok((k, v))) = iter.next() {
            let geo_hash = u64::from_be_bytes(k.as_bytes().try_into().unwrap());
            //如果已经存在，不需要替换
            if exist_geo_hashs.contains(&geo_hash) {
                continue;
            }
            if !meshes.contains_key(&geo_hash) {
                let aabb = Aabb::from_bytes(v.as_bytes())?;
                let mesh = mgr.get_mesh_from_localdb(geo_hash).expect("read mesh from local db error.");
                meshes.insert(geo_hash, PlantGeoData {
                    geo_hash,
                    mesh: Some(mesh),
                    aabb: Some(aabb),
                });
            }
        }
        println!("合并缓存后mesh数量：{}", meshes.len());
    }


    for chunk in &meshes.iter().chunks(1000) {
        for k in chunk {
            let json = serde_json::to_value(k.1).unwrap();
            data.push(json);
        }
        let aql = if replace {
            AqlQuery::new(r#"
            with @@collection
            LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection  OPTIONS { ignoreErrors: true , overwriteMode: "replace" }"#)
                .bind_var("@collection", collection)
                .bind_var("elements", take(&mut data))
        } else {
            AqlQuery::new(r#"
            with @@collection
            LET data = @elements
                    FOR d IN data
                        INSERT d INTO @@collection  OPTIONS { ignoreErrors: true , overwriteMode: "ignore" }"#)
                .bind_var("@collection", collection)
                .bind_var("elements", take(&mut data))
        };
        database.aql_query::<Vec<()>>(aql).await.unwrap();
    }

    Ok(())
}