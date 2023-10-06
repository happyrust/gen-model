use std::{env, fs};
use std::collections::HashMap;
use std::io::Write;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance, DataCenterProject};
use aios_core::options::DbOption;
use aios_core::pdms_types::RefU64;
use bb8_arangodb::arangors_lite::Database;
use sqlx::{MySql, Pool};
use crate::api::children::query_owner_till_type;
use crate::api::element::{query_name, query_owner_from_id};
use crate::aql_api::children::{query_ancestor_name_of_type_aql, query_owner_with_type_aql, query_refnos_travel_children_with_type_aql, query_travel_children_aql};
use crate::aql_api::pdms_room::{query_room_name_from_refno_aql, query_room_name_from_refnos_aql};
use crate::data_center_api::data_api::{get_quarantine_room_name, get_refno_latest_version};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::ArDatabase;
use crate::test::common::get_arangodb_conn_from_db_option_for_test;

pub async fn get_valv_data(refnos: Vec<RefU64>, database: &ArDatabase, pool: &Pool<MySql>) -> anyhow::Result<DataCenterProject> {
    let mut instance = Vec::new();
    if let Ok(valves) = query_refnos_travel_children_with_type_aql(database,
                                                                   &refnos, vec!["VALV".to_string()]).await {
        // 查询所有需要的房间号
        let refnos = valves.iter().map(|child| child.refno).collect::<Vec<RefU64>>();
        let room_map = query_room_name_from_refnos_aql(refnos, &database).await?;
        let room_map = room_map.into_iter()
            .map(|x| (x.refno, x.room_name))
            .collect::<HashMap<RefU64, String>>();

        for valv in valves {
            let room_name = room_map.get(&valv.refno).unwrap_or(&"".to_string()).clone();
            instance.push(DataCenterInstance {
                object_model_code: "COMPBA".to_string(),
                project_code: "1516".to_string(),
                instance_code: if valv.name.starts_with("/") { valv.name[1..].to_string() } else { valv.name },
                version: get_refno_latest_version(),
                attributes: vec![DataCenterAttr {
                    attribute_model_code: "COMP8".to_string(),
                    value: room_name,
                }],
            });
        }
    }
    Ok(DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: "1516".to_string(),
        owner: "KY1801".to_string(),
        instances: instance,
    })
}

/// 获取防火阀数据 通风专业
pub async fn get_tf_fireproof_valv_data(refnos: Vec<RefU64>, aios_mgr:&AiosDBManager) -> anyhow::Result<DataCenterProject> {
    let mut instance = Vec::new();
    let database = aios_mgr.get_arango_db().await?;
    if let Ok(valves) = query_refnos_travel_children_with_type_aql(&database, &refnos,
                                                                   vec!["DAMP".to_string()]).await {
        for valv in valves {
            let name = if valv.name.starts_with("/") { valv.name[1..].to_string() } else { valv.name };
            if name.len() < 10 { continue; }
            let mut attr = Vec::new();
            let replace_name = name.replace("-", "");
            attr.push(DataCenterAttr {
                attribute_model_code: "COMP1".to_string(),
                value: AttrValue::AttrString(replace_name[..1].to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "COMP2".to_string(),
                value: AttrValue::AttrString(replace_name[1..4].to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "COMP3".to_string(),
                value: AttrValue::AttrString(replace_name[4..7].to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "COMP4".to_string(),
                value: AttrValue::AttrString(replace_name[7..10].to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "COMP6".to_string(),
                value: AttrValue::AttrString(name.to_string()).into(),
            });
            // let bran_name = query_name(valv.owner,pool).await?;
            // let bran_name_split = bran_name.split("-").collect::<Vec<_>>();
            // let mut hvac_name = "".to_string();
            // for bran_name in bran_name_split {
            //     if bran_name.len() == 1 && bran_name != "/" {
            //         hvac_name = bran_name.to_string();
            //     }
            //     if !hvac_name.is_empty() { break; }
            // }
            // attr.push(DataCenterAttr {
            //     attribute_model_code: "HVAC26".to_string(),
            //     value: AttrValue::AttrString(hvac_name).into(),
            // });
            let room_name = get_quarantine_room_name(valv.refno, &database).await?;
            let Some(zone_name) = query_ancestor_name_of_type_aql(&database, valv.refno, "ZONE").await? else { continue; };

            let mut room_model_code = "COMPBBA2".to_string();
            let mut object_model_code = "COMPBBA".to_string();
            if zone_name.contains("VES") {
                room_model_code = "COMPBBB1".to_string();
                object_model_code = "COMPBBB".to_string();
            }
            attr.push(DataCenterAttr {
                attribute_model_code: room_model_code,
                value: AttrValue::AttrStrArray(vec![room_name.0, room_name.1]).into(),
            });
            instance.push(DataCenterInstance {
                object_model_code,
                project_code: aios_mgr.db_option.project_code.to_string(),
                instance_code: name,
                version: get_refno_latest_version(),
                attributes: attr,
            });
        }
    }
    Ok(DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: "1516".to_string(),
        owner: "KY1801".to_string(),
        instances: instance,
    })
}


#[tokio::test]
async fn test_valv() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let refno = RefU64::from_refno_str("24383/67619").unwrap();
    let pool = AiosDBManager::get_db_pool(&url, "avevamarinesample").await?;
    let data = get_valv_data(vec![refno], &database, &pool).await.unwrap();
    let mut file = fs::File::create("阀门.json")?;
    let data = serde_json::to_string(&data).unwrap();
    file.write_all(&data.into_bytes())?;
    Ok(())
}