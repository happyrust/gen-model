use std::fs;
use std::io::Write;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance, DataCenterProject};
use aios_core::options::DbOption;
use aios_core::pdms_types::RefU64;
use arangors_lite::Database;
use crate::aql_api::children::query_travel_children_with_type_aql;
use crate::graph_db::pdms_arango::get_arangodb_conn_from_db_option;

/// 获得机械设备的数据
pub async fn get_machine_equi_data(refno: RefU64,database:&Database) -> Vec<DataCenterInstance> {
    let mut result = Vec::new();
    if let Ok(children) = query_travel_children_with_type_aql(database,refno,"EQUI").await {
        for child in children {
            let name = if child.name.starts_with("/") { child.name[1..].to_string() } else { child.name };
            let mut attr = Vec::new();
            attr.push(DataCenterAttr {
                attribute_model_code: "COMP".to_string(),
                value: AttrValue::AttrString("Test".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "COMPB3".to_string(),
                value: AttrValue::AttrFloat(0.0).into(),
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
    result
}

#[tokio::test]
async fn test_get_machine_equi_data() -> anyhow::Result<()>{
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option(&db_option).await?;
    let refno = RefU64::from_refno_str("23584/107").unwrap();
    let single_data = get_machine_equi_data(refno,&database).await;
    let project = DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: "1516".to_string(),
        owner: "KY1801".to_string(),
        instances: single_data,
    };
    let mut file = fs::File::create("机械设备.json").unwrap();
    let data = serde_json::to_string(&project).unwrap();
    file.write_all(&data.into_bytes()).unwrap();
    Ok(())
}