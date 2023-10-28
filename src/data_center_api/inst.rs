use std::collections::HashMap;
use std::fs;
use std::io::Write;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance, DataCenterProject};
use aios_core::options::DbOption;
use aios_core::pdms_types::RefU64;
use bb8_arangodb::arangors_lite::Database;
use sqlx::{MySql, Pool};
use crate::api::refno_info::query_refno_height_position;
use crate::aql_api::children::{query_refnos_travel_children_with_type_aql};
use crate::aql_api::pdms_room::{query_room_name_from_refno_aql, query_room_name_from_refnos_aql};
use crate::data_center_api::data_api::get_refno_latest_version;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::arangodb::ArDatabase;

pub async fn get_inst_data(refnos: Vec<RefU64>, database: &ArDatabase, pool: &Pool<MySql>) -> anyhow::Result<DataCenterProject> {
    let mut instance = Vec::new();
    if let Ok(valves) = query_refnos_travel_children_with_type_aql(&database, &refnos,
                                                                   vec!["INST".to_string(), "EQUI".to_string()]).await {
        // 查询所有需要的房间号
        let refnos = valves.iter().map(|child| child.refno).collect::<Vec<RefU64>>();
        let room_map = query_room_name_from_refnos_aql(refnos, &database).await?;
        let room_map = room_map.into_iter()
            .map(|x| (x.refno, x.room_name))
            .collect::<HashMap<RefU64, String>>();

        for valv in valves {
            let room_name = room_map.get(&valv.refno).unwrap_or(&"".to_string()).clone();
            let position = query_refno_height_position(valv.refno, pool).await?;
            instance.push(DataCenterInstance {
                object_model_code: "COMPADD".to_string(),
                project_code: "1516".to_string(),
                instance_code: if valv.name.starts_with("/") { valv.name[1..].to_string() } else { valv.name },
                version: get_refno_latest_version(),
                attributes: vec![DataCenterAttr {
                    attribute_model_code: "COMP8".to_string(),
                    value: room_name,
                }, DataCenterAttr {
                    attribute_model_code: "COMPADD47".to_string(),
                    value: position.to_string(),
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


/// 获取仪控设备的信息
pub async fn get_inst_equi_data(refnos: Vec<RefU64>, aios_mgr: &AiosDBManager) -> anyhow::Result<DataCenterProject> {
    let mut instance = Vec::new();
    let database = aios_mgr.get_arango_db().await?;
    if let Ok(equis) = query_refnos_travel_children_with_type_aql(&database, &refnos,
                                                                  vec!["EQUI".to_string()]).await {
        for equi in equis {
            let name = if equi.name.starts_with("/") { equi.name[1..].to_string() } else { equi.name };
            if name.len() < 9 { continue; };
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
                value: AttrValue::AttrString(replace_name[7..9].to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "COMP6".to_string(),
                value: AttrValue::AttrString(name.to_string()).into(),
            });
            let room_name = query_room_name_from_refno_aql(equi.refno, &database).await?.unwrap_or("".to_string());
            let position = aios_mgr.get_world_transform(equi.refno)?;
            attr.push(DataCenterAttr {
                attribute_model_code: "COMP8".to_string(),
                value: AttrValue::AttrString(room_name).into(),
            });
            if let Some(position) = position {
                attr.push(DataCenterAttr {
                    attribute_model_code: "COMPAD34".to_string(),
                    value: AttrValue::AttrFloat(position.translation.z).into(),
                });
            } else {
                attr.push(DataCenterAttr {
                    attribute_model_code: "COMPAD34".to_string(),
                    value: AttrValue::AttrFloat(0.0).into(),
                });
            }
            instance.push(DataCenterInstance {
                object_model_code: "COMPAD".to_string(),
                project_code: "1516".to_string(),
                instance_code: name,
                version: get_refno_latest_version(),
                attributes: attr,
            });
        }
    }
    Ok(DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        owner: "KY1801".to_string(),
        instances: instance,
    })
}

// #[tokio::test]
// async fn test_valv() -> anyhow::Result<()> {
//     use config::{Config, ConfigError, Environment, File};
//     let s = Config::builder()
//         .add_source(File::with_name("DbOption"))
//         .build()?;
//     let db_option: DbOption = s.try_deserialize().unwrap();
//     let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
//     let refno = RefU64::from_refno_str("24381/104050").unwrap();
//     let data = get_inst_data(refno, &database).await;
//     let mut file = fs::File::create("仪控.json")?;
//     let data = serde_json::to_string(&data).unwrap();
//     file.write_all(&data.into_bytes())?;
//     Ok(())
// }