use std::{env, fs};
use std::io::Write;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance, DataCenterProject};
use aios_core::options::DbOption;
use aios_core::pdms_types::RefU64;
use bb8_arangodb::arangors::Database;
use sqlx::{MySql, Pool};
use crate::api::refno_info::query_refno_height_position;
use crate::api::room_code::query_room_code;
use crate::aql_api::children::{query_refnos_travel_children_with_type_aql};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::ArDatabase;

/// 获得机械设备的数据
pub async fn get_machine_equi_data(refnos: Vec<RefU64>,pool:&Pool<MySql>,database:&ArDatabase) -> anyhow::Result<DataCenterProject> {
    let mut result = Vec::new();
    if let Ok(children) = query_refnos_travel_children_with_type_aql(database,refnos,vec!["EQUI"]).await {
        for child in children {
            let name = if child.name.starts_with("/") { child.name[1..].to_string() } else { child.name };
            let mut attr = Vec::new();
            let room_name = query_room_code(child.refno,pool).await?.unwrap_or("".to_string());
            let position = query_refno_height_position(child.refno,pool).await?;
            attr.push(DataCenterAttr {
                attribute_model_code: "COMP8".to_string(),
                value: AttrValue::AttrString(room_name).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "COMPB3".to_string(),
                value: AttrValue::AttrString(position).into(),
            });
            result.push(DataCenterInstance {
                object_model_code: "COMPB".to_string(),
                project_code: "1516".to_string(),
                instance_code: name,
                version: "A版".to_string(),
                attributes: attr,
            });
        }
    }
    let project = DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: "1516".to_string(),
        owner: "KY1801".to_string(),
        instances: result,
    };
    Ok(project)
}

#[tokio::test]
async fn test_get_machine_equi_data() -> anyhow::Result<()>{
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option(&db_option).await?;
    let refno = RefU64::from_refno_str("23584/107").unwrap();
    let pool = AiosDBManager::get_db_pool(&url, "avevamarinesample").await?;
    let project = get_machine_equi_data(vec![refno],&pool,&database).await.unwrap();
    let mut file = fs::File::create("机械设备.json").unwrap();
    let data = serde_json::to_string(&project).unwrap();
    file.write_all(&data.into_bytes()).unwrap();
    Ok(())
}