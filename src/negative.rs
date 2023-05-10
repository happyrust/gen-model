use std::collections::HashMap;
use aios_core::negative_mesh_type::{NegativeEdges, NegativeEles};
use aios_core::pdms_types::{GeoHash, PdmsElement, RefU64};
use aios_core::shape::pdms_shape::PdmsMesh;
use arangors_lite::{AqlQuery, Database};
use bevy::prelude::{Transform};
use glam::{Mat4, Vec3};
use itertools::Itertools;
use sqlx::{MySql, Pool};
use crate::aql_api::pdms_mesh::{query_catr_refnos_meshes_aql, query_refnos_meshes_aql};
use crate::aql_api::{convert_refno_vec_from_vec_string, PdmsElementAql};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::{AiosDBManager, PRIMITIVE_NOUN_NAMES};
use crate::graph_db::pdms_arango::{get_arangodb_conn_from_db_option, save_arangodb_with_database};
use csg::{Mesh, Pt3};
use dashmap::{DashMap, DashSet};
use parry3d::bounding_volume::Aabb;
use crate::api::element::query_refno_type;
use crate::aql_api::foreign_refnos::query_foreign_refno_aql;
use crate::graph_db::pdms_inst_arango::query_rvm_instance_data_from_refno_aql;
use std::io::Write;
use csg::mesh::IndexedMesh;
use crate::consts::AQL_PDMS_ELES_COLLECTION;

/// 查找需要负实体计算的instance
pub async fn query_instance_refnos_negative_aql(refno:RefU64,database:&Database) -> anyhow::Result<Vec<RefU64>> {
    let id = format!("{AQL_PDMS_ELES_COLLECTION}/{}",refno.to_url_refno());
    let aql = AqlQuery::new("
    for v in 0..10 inbound @id pdms_edges
        filter !POSITION(['NCYL' ,'NBOX','NCON', 'NSNO','NPYR', 'NDIS' ,'NXTR', 'NCTO' ,'NRTO' ,'NSLC','NREV'],v.noun)
        filter document('pdms_instances',v._key) != null
        return v._key
    ").bind_var("id",id);
    let result:Vec<String> = database.aql_query(aql).await?;
    let refnos = convert_refno_vec_from_vec_string(result);
    Ok(refnos)
}

async fn boolean_negative_mesh(refno: RefU64, aios_mgr: &AiosDBManager) -> anyhow::Result<()> {
    let mut negative_mesh_map = DashMap::new();
    let database = aios_mgr.get_arangodb_conn().await?;
    let need_compute_refnos = query_negative_refnos_aql(refno, aios_mgr, &database).await?;
    for (refno, negative_refnos) in need_compute_refnos {
        if let Some((_,pool)) = aios_mgr.get_project_pool_by_refno(refno).await {
            if let Some((refno, mesh)) = compute_boolean_mesh(refno, negative_refnos, &pool, &database).await? {
                negative_mesh_map.entry(refno).or_insert(mesh);
            }
        }
    }
    save_boolean_negative_mesh(negative_mesh_map, &database).await?;
    Ok(())
}

pub async fn save_boolean_negative_mesh(negative_mesh_map: DashMap<RefU64, IndexedMesh>, database: &Database) -> anyhow::Result<()> {
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
pub async fn compute_boolean_mesh(refno: RefU64, negative_elements: Vec<PdmsElement>, pool: &Pool<MySql>, database: &Database) -> anyhow::Result<Option<(RefU64, IndexedMesh)>> {
    let refno_type = query_refno_type(refno, pool).await?;
    let refno_meshes_map = query_catr_refnos_meshes_aql(refno, database).await?;
    let negative_refnos = negative_elements.clone().iter().filter_map(|x| RefU64::from_refno_str(&x.refno).ok()).collect::<Vec<_>>();
    let transform = query_rvm_instance_data_from_refno_aql(refno, database).await?;
    if let Some(refno_geo_info) = transform {
        let mut refno_csg_mesh = Mesh::default();
        let refno_transform = Transform {
            translation: refno_geo_info.world_transform.1,
            rotation: refno_geo_info.world_transform.0,
            scale: refno_geo_info.world_transform.2,
        };
        if PRIMITIVE_NOUN_NAMES.contains(&refno_type.as_str()) {
            // 找到基本体的mesh
            if let Some(refno_mesh) = refno_meshes_map.get(&refno) {
                for data in refno_geo_info.data {
                    let t = refno_transform * Transform {
                        translation: data.transform.1,
                        rotation: data.transform.0,
                        scale: data.transform.2,
                    };
                    refno_csg_mesh += refno_mesh.value().into_csg_mesh(&t);
                    // 减去负实体的 mesh
                    for negative_refno in &negative_refnos {
                        if let Some(negative_mesh) = refno_meshes_map.get(&negative_refno) {
                            let transform = query_rvm_instance_data_from_refno_aql(*negative_refno, &database).await?;
                            if let Some(geo_info) = transform {
                                let negative_transform = Transform {
                                    translation: geo_info.world_transform.1,
                                    rotation: geo_info.world_transform.0,
                                    scale: geo_info.world_transform.2,
                                };
                                for data in geo_info.data {
                                    let transform = data.clone().transform;
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
                    }
                }
            }
        } else {
            for data in &refno_geo_info.data {
                if let Some(refno_mesh) = refno_meshes_map.get(&data.refno) {
                    let t = refno_transform * Transform {
                        translation: data.transform.1,
                        rotation: data.transform.0,
                        scale: data.transform.2,
                    };
                    // let mut refno_csg_mesh = refno_mesh.value().into_csg_mesh(&t);
                    if negative_refnos.contains(&data.refno) {
                        refno_csg_mesh += refno_mesh.value().into_csg_mesh(&t);
                    } else {
                        refno_csg_mesh -= refno_mesh.value().into_csg_mesh(&t);
                    }
                }
            }
        }
        let index_mesh = refno_csg_mesh.simplified(0.1);
        return Ok(Some((refno,index_mesh)));
    }
    Ok(None)
}

pub async fn query_negative_refnos_aql(refno: RefU64, aios_mgr: &AiosDBManager, database: &Database) -> anyhow::Result<HashMap<RefU64, Vec<PdmsElement>>> {
    let mut map = HashMap::new();
    let attr = aios_mgr.get_implicit_attr(refno, Some(vec!["SPRE", "CATR"])).await?;
    let spre = attr.get_refu64("SPRE").unwrap_or(RefU64(0));
    let catr = attr.get_refu64("CATR").unwrap_or(RefU64(0));

    let catr = if catr.0 != 0 {
        let gmre = query_foreign_refno_aql(refno, vec!["CATR", "GMRE"], database).await?;
        if let Some(gmre) = gmre {
            gmre
        } else {
            refno
        }
    } else if spre.0 != 0 {
        let catr = query_foreign_refno_aql(refno, vec!["SPRE", "GMRE"], database).await?;
        if let Some(catr) = catr {
            catr
        } else {
            refno
        }
    } else {
        refno
    };

    let key = format!("{}/{}", "pdms_eles", catr.to_url_refno());
    let aql = AqlQuery::new("
    for c in 1..1000 inbound @id pdms_edges
    filter length(
        for z in 1 inbound c._id pdms_edges
            return 1
        ) == 0
    filter c.noun in ['NCYL' ,'NBOX','NCON', 'NSNO','NPYR', 'NDIS' ,'NXTR', 'NCTO' ,'NRTO' ,'NSLC','NREV','NLCY',
     'NLPY', 'NLSN', 'NSBO', 'NSCO', 'NSCT', 'NSCY', 'NSDS', 'NSEX' ,'NSRE', 'NSRT', 'NSSL', 'NSSP']
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
            map.entry(refno).or_insert_with(Vec::new).push(
                pdms_element);
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
    let database = aios_mgr.get_arangodb_conn().await?;
    let refno = RefU64::from_refno_str("23584/5382").unwrap();
    for refno in query_instance_refnos_negative_aql(refno,&database).await? {
        boolean_negative_mesh(refno, &aios_mgr).await?;
    }
    // let result = compute_boolean_mesh(refno, negative_refnos, &aios_mgr).await?;
    Ok(())
}