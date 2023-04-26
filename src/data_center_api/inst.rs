use std::fs;
use std::io::Write;
use aios_core::data_center::{DataCenterAttr, DataCenterInstance, DataCenterProject};
use aios_core::options::DbOption;
use aios_core::pdms_types::RefU64;
use arangors_lite::Database;
use sqlx::{MySql, Pool};
use crate::api::refno_info::query_refno_height_position;
use crate::api::room_code::query_room_code;
use crate::aql_api::children::{query_refnos_travel_children_with_type_aql};
use crate::graph_db::pdms_arango::get_arangodb_conn_from_db_option;

pub async fn get_inst_data(refnos: Vec<RefU64>, database: &Database,pool:&Pool<MySql>) -> anyhow::Result<DataCenterProject> {
    let mut instance = Vec::new();
    if let Ok(valves) = query_refnos_travel_children_with_type_aql(database, refnos, vec!["INST","EQUI"]).await {
        for valv in valves {
            let room_name = query_room_code(valv.refno,pool).await?.unwrap_or("".to_string());
            let position = query_refno_height_position(valv.refno,pool).await?;
            instance.push(DataCenterInstance {
                object_model_code: "COMPADD".to_string(),
                project_code: "1516".to_string(),
                instance_code: if valv.name.starts_with("/") { valv.name[1..].to_string() } else { valv.name },
                version: "A版".to_string(),
                attributes: vec![DataCenterAttr {
                    attribute_model_code: "COMP8".to_string(),
                    value: room_name,
                },DataCenterAttr{
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

// #[tokio::test]
// async fn test_valv() -> anyhow::Result<()> {
//     use config::{Config, ConfigError, Environment, File};
//     let s = Config::builder()
//         .add_source(File::with_name("DbOption"))
//         .build()?;
//     let db_option: DbOption = s.try_deserialize().unwrap();
//     let database = get_arangodb_conn_from_db_option(&db_option).await?;
//     let refno = RefU64::from_refno_str("24381/104050").unwrap();
//     let data = get_inst_data(refno, &database).await;
//     let mut file = fs::File::create("仪控.json")?;
//     let data = serde_json::to_string(&data).unwrap();
//     file.write_all(&data.into_bytes())?;
//     Ok(())
// }