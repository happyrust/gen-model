use std::io::Write;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance};
use aios_core::data_center::AttrValue::{AttrFloat, AttrString, AttrVec3};
use aios_core::pdms_types::*;
use aios_core::tool::math_tool::quat_to_pdms_ori_str;
use dashmap::DashMap;
use crate::api::element::{query_ele_node, query_name};
use crate::aql_api::foreign_refnos::query_foreign_name_aql;
use crate::aql_api::pdms_room::query_room_name_from_refno_aql;
use crate::data_center_api::auto_get_attr::get_material_map_from_code;
use crate::data_center_api::data_api::{get_bran_itema_attr, get_material_pressure_code, get_refno_latest_version, get_spre_material_code};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::arangodb::ArDatabase;

pub async fn get_data_center_flan_attr(refno: PdmsElement, bran_name: &str, room_code: String,
                                       database: &ArDatabase, aios_mgr: &AiosDBManager) -> DataCenterInstance {
    let need_query_material_code = vec![("ITEMA11".to_string(), "Code".to_string()),
                                        ("ITEMA12".to_string(), "Name".to_string()), ("ITEMA13".to_string(), "Make".to_string()),
                                        ("ITEMA14".to_string(), "Mat".to_string()),
                                        ("ITEMA15".to_string(), "MatSpec".to_string()),
                                        ("ITEMA16".to_string(), "Spec".to_string()),
                                        ("ITEMA17".to_string(), "RCCM".to_string()),
                                        ("ITEMA18".to_string(), "QAGrade".to_string()),
                                        ("ITEMA19".to_string(), "Weight".to_string()), ("ITEMAD5".to_string(), "Diameter".to_string()),
                                        ("ITEMAD7".to_string(), "Link".to_string())];
    let mut result = Vec::new();
    // 重复的取值
    get_bran_itema_attr(refno.clone(), bran_name, room_code, database, aios_mgr, &mut result).await;

    let spre_name = query_foreign_name_aql(refno.refno, vec!["SPRE", "SPRE"], database).await.unwrap_or(None).unwrap_or("".to_string());
    let material_code = get_spre_material_code(&spre_name).unwrap_or("".to_string());
    let material_map = if let Ok(puhua_pool) = aios_mgr.get_puhua_pool().await {
        let query_code = need_query_material_code.iter().map(|x| x.1.clone()).collect::<Vec<_>>();
        let material_map = get_material_map_from_code(&material_code, query_code, &puhua_pool).await;
        material_map
    } else {
        DashMap::default()
    };
    for (item_code, material_code) in &need_query_material_code {
        let material = if material_map.contains_key(material_code) {
            material_map.get(material_code).unwrap().value().clone()
        } else {
            "".to_string()
        };
        result.push(DataCenterAttr {
            attribute_model_code: item_code.to_string(),
            value: material,
        });
    }
    get_material_pressure_code("ITEMAE4", "ITEMAE5", "ITEMAE7", &mut result, &material_map);
    DataCenterInstance {
        object_model_code: "ITEMAE".to_string(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        instance_code: refno.name,
        version: get_refno_latest_version(),
        attributes: result,
    }
}

#[tokio::test]
async fn test_get_data_center_flan_attr() -> anyhow::Result<()> {
    let aios_mgr = AiosDBManager::init_form_config().await?;
    let database = aios_mgr.get_arango_db().await?;
    let tee_refno = RefU64::from_str("24383/66752").unwrap();
    let pool = aios_mgr.get_project_pool_by_refno(tee_refno).await.unwrap();
    let tee_node = query_ele_node(tee_refno, &pool.1).await.unwrap();
    let owner_name = query_name(tee_node.owner, &pool.1).await.unwrap();

    let result = get_data_center_flan_attr(tee_node.into(), &owner_name, "".to_string(),&database, &aios_mgr).await;
    let mut file = std::fs::File::create("tee.json")?;
    let json = serde_json::to_vec(&result)?;
    file.write_all(&json)?;
    Ok(())
}