use std::collections::HashMap;
use aios_core::negative_mesh_type::{NegativeEdges, NegativeEles};
use aios_core::pdms_types::{GeoHash, PdmsElement, RefU64};
use aios_core::shape::pdms_shape::PdmsMesh;
use arangors_lite::{AqlQuery, Database};
use bevy::prelude::{Transform};
use glam::{Mat4, Vec3};
use itertools::Itertools;
use sqlx::{MySql, Pool};
use crate::aql_api::pdms_mesh::{query_refnos_meshes_aql};
use crate::aql_api::PdmsElementAql;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::{get_arangodb_conn_from_db_option, save_arangodb_with_database};
use crate::options::DbOption;
use csg::{Mesh , Pt3};
use dashmap::DashMap;
use parry3d::bounding_volume::Aabb;
use crate::graph_db::pdms_inst_arango::query_rvm_instance_data_from_refno_aql;

async fn boolean_negative_mesh(refno: RefU64, aios_mgr:&AiosDBManager) -> anyhow::Result<()> {
    let mut negative_mesh_map = DashMap::new();
    let database = aios_mgr.get_arangodb_conn().await?;
    let need_compute_refnos = query_negative_refnos_aql(refno, &database).await?;
    for (refno, negative_refnos) in need_compute_refnos {
        if let Some((refno,mesh)) = compute_boolean_mesh(refno,negative_refnos,aios_mgr).await?{
            negative_mesh_map.entry(refno).or_insert(mesh);
        }
    }
    save_boolean_negative_mesh(negative_mesh_map,&database).await?;
    Ok(())
}

pub async fn save_boolean_negative_mesh(negative_mesh_map:DashMap<RefU64,PdmsMesh>, database: &Database) -> anyhow::Result<()> {
    let mut eles_vec = Vec::new();
    let eles = negative_mesh_map.into_iter().collect::<Vec<_>>();
    for (refno, mesh) in eles {
        let mesh_encode = hex::encode(&mesh.into_compress_bytes());
        eles_vec.push(NegativeEles {
            _key: refno.to_url_refno(),
            mesh: mesh_encode,
        })
    }
    if let Ok(eles_json) = serde_json::to_value(&eles_vec) {
        let _ = save_arangodb_with_database(eles_json, "negative_eles", database).await;
    }
    Ok(())
}

// refno : 基本体的 refno  negative_refnos ： 负实体的集合
pub async fn compute_boolean_mesh(refno: RefU64, negative_refnos: Vec<PdmsElement>, aios_mgr: &AiosDBManager) -> anyhow::Result<Option<(RefU64, PdmsMesh)>> {
    let database = aios_mgr.get_arangodb_conn().await?;
    let refno_meshes_map = query_refnos_meshes_aql(refno, &database).await?;
    let refno_transform = aios_mgr.get_world_transform(refno).await?;
    if let Some(mut refno_transform) = refno_transform {
        if let Some(mut refno_mesh) = refno_meshes_map.get_mut(&refno) {
            let transform = query_rvm_instance_data_from_refno_aql(refno, &database).await?;
            if let Some(geo_info) = transform {
                let transform = geo_info.data[0].clone().transform;
                let t = refno_transform * Transform {
                    translation: transform.1,
                    rotation: transform.0,
                    scale: transform.2,
                };
                let mut refno_csg_mesh = refno_mesh.value().into_csg_mesh(&t);
                // 计算基本体下面的负实体的 mesh
                for negative_refno in negative_refnos {
                    let negative_refno = RefU64::from_refno_str(&negative_refno.refno).unwrap();
                    if let Some(negative_mesh) = refno_meshes_map.get(&negative_refno) {
                        let negative_transform = aios_mgr.get_world_transform(negative_refno).await?;
                        if negative_transform.is_none() {
                            dbg!("transform none");
                            continue;
                        }
                        let mut negative_transform = negative_transform.unwrap();
                        let transform = query_rvm_instance_data_from_refno_aql(negative_refno, &database).await?;
                        if let Some(geo_info) = transform {
                            let transform = geo_info.data[0].clone().transform;
                            let t = negative_transform * Transform {
                                translation: transform.1,
                                rotation: transform.0,
                                scale: transform.2,
                            };
                            let negative_csg_mesh = negative_mesh.into_csg_mesh(&t);
                            refno_csg_mesh -= negative_csg_mesh;
                        }
                    }
                }
                let pdms_mesh = refno_mesh.from_scg_mesh(&refno_csg_mesh,&refno_transform);
                return Ok(Some((refno,pdms_mesh)))
            }
        }
    }
    Ok(None)
}

pub async fn query_negative_refnos_aql(refno: RefU64, database: &Database) -> anyhow::Result<HashMap<RefU64, Vec<PdmsElement>>> {
    let mut map = HashMap::new();
    let key = format!("{}/{}", "pdms_eles", refno.to_url_refno());
    let aql = AqlQuery::new("
    for c in 1..1000 inbound @id pdms_edges
    filter length(
        for z in 1 inbound c._id pdms_edges
            return 1
        ) == 0
    filter c.noun in ['NCYL' ,'NBOX','NCON', 'NSNO','NPYR', 'NDIS' ,'NXTR', 'NCTO' ,'NRTO' ,'NSLC','NREV']
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
    // use config::{Config, ConfigError, Environment, File};
    // let s = Config::builder()
    //     .add_source(File::with_name("DbOption"))
    //     .build()?;
    // let db_option: DbOption = s.try_deserialize().unwrap();
    let aios_mgr = AiosDBManager::init_form_config().await?;
    // let database = aios_mgr.get_arangodb_conn().await?;
    let refno = RefU64::from_refno_str("23584/5386").unwrap();
    boolean_negative_mesh(refno, &aios_mgr).await?;
    // let result = compute_boolean_mesh(refno, negative_refnos, &aios_mgr).await?;
    Ok(())
}