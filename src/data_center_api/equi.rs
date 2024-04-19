use std::{env, fs};
use std::io::Write;
use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance, DataCenterProject};
use aios_core::options::DbOption;
use aios_core::pdms_types::RefU64;

use sqlx::{MySql, Pool};
use crate::api::refno_info::query_refno_height_position;
use crate::aql_api::children::{query_ancestor_name_of_type_aql, query_owner_with_type_aql, query_refnos_travel_children_with_type_aql};
use crate::aql_api::pdms_room::query_room_name_from_refno_aql;
use crate::data_center_api::data_api::get_refno_latest_version;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::arangodb::ArDatabase;
use crate::test::common::get_arangodb_conn_from_db_option_for_test;

/// 获得工艺设备的数据
pub async fn get_gy_equi_data(refnos: Vec<RefU64>, database: &ArDatabase) -> anyhow::Result<DataCenterProject> {
    let mut result = Vec::new();
    if let Ok(children) = query_refnos_travel_children_with_type_aql(database, &refnos, vec!["EQUI".to_string()]).await {
        for child in children {
            let mut attr = Vec::new();
            if !split_equi_name(&child.name, &mut attr) { continue; }
            let room_name = query_room_name_from_refno_aql(child.refno, &database).await?.unwrap_or("".to_string());
            // let position = query_refno_height_position(child.refno, pool).await?;
            attr.push(DataCenterAttr {
                attribute_model_code: "COMP8".to_string(),
                value: AttrValue::AttrString(room_name).into(),
            });
            // attr.push(DataCenterAttr {
            //     attribute_model_code: "COMPB3".to_string(),
            //     value: AttrValue::AttrString(position).into(),
            // });
            result.push(DataCenterInstance {
                object_model_code: "COMPB".to_string(),
                project_code: "1516".to_string(),
                instance_code: child.name,
                version: get_refno_latest_version(),
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

/// 获取消防栓的信息 给排水专业
pub async fn get_sg_fire_hydrant_equi_data(refnos: Vec<RefU64>, aios_mgr:&AiosDBManager) -> anyhow::Result<DataCenterProject> {
    let mut result = Vec::new();
    let database = aios_mgr.get_arango_db().await?;
    if let Ok(children) = query_refnos_travel_children_with_type_aql(&database, &refnos, vec!["EQUI".to_string()]).await {
        for child in children {
            if !child.name.ends_with("RJ") { continue; };
            let name = if child.name.starts_with("/") { child.name[1..].to_string() } else { child.name };
            let mut attr = Vec::new();
            if !split_equi_name(&name, &mut attr) { continue; }
            let room_name = query_room_name_from_refno_aql(child.refno, &database).await?.unwrap_or("".to_string());
            attr.push(DataCenterAttr {
                attribute_model_code: "COMP8".to_string(),
                value: AttrValue::AttrString(room_name).into(),
            });
            // attr.push(DataCenterAttr {
            //     attribute_model_code: "COMPB3".to_string(),
            //     value: AttrValue::AttrString(position).into(),
            // });
            result.push(DataCenterInstance {
                object_model_code: "COMPBTHA".to_string(),
                project_code: aios_mgr.db_option.project_code.to_string(),
                instance_code: name,
                version: get_refno_latest_version(),
                attributes: attr,
            });
        }
    }
    let project = DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        owner: "KY1801".to_string(),
        instances: result,
    };
    Ok(project)
}

/// 获取电气专业贯穿件信息
pub async fn get_dq_cross_element_data(refnos: Vec<RefU64>, aios_mgr: &AiosDBManager) -> anyhow::Result<DataCenterProject> {
    let mut result = Vec::new();
    let database = aios_mgr.get_arango_db().await?;
    if let Ok(children) = query_refnos_travel_children_with_type_aql(&database, &refnos, vec!["EQUI".to_string()]).await {
        for child in children {
            if !child.name.contains("ZZZ") && child.name.len() < 4 { continue; };
            let mut attr = Vec::new();
            let machine_num = get_site_name_first_char(child.refno, &database).await?;
            let name = if child.name.starts_with("/") { child.name[1..].to_string() } else { child.name.to_string() };
            attr.push(DataCenterAttr {
                attribute_model_code: "COMP1".to_string(),
                value: AttrValue::AttrString(machine_num).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "COMP2".to_string(),
                value: AttrValue::AttrString(name[..3].to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "COMP3".to_string(),
                value: AttrValue::AttrString(name[4..].to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "COMP4".to_string(),
                value: AttrValue::AttrString("".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "COMP6".to_string(),
                value: AttrValue::AttrString("电气贯穿件".to_string()).into(),
            });
            let position = aios_mgr.get_world_transform(child.refno).await?.unwrap_or_default();
            attr.push(DataCenterAttr {
                attribute_model_code: "COMPB3".to_string(),
                value: AttrValue::AttrFloat(position.translation.z).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "COMPBSDB31".to_string(),
                value: AttrValue::AttrString("".to_string()).into(),
            });
            result.push(DataCenterInstance {
                object_model_code: "COMPBSDB".to_string(),
                project_code: aios_mgr.db_option.project_code.to_string(),
                instance_code: child.name,
                version: get_refno_latest_version(),
                attributes: attr,
            });
        }
    }
    let project = DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        owner: "KY1801".to_string(),
        instances: result,
    };
    Ok(project)
}

/// 获取site name 的 第一个字符 ，即机组号
async fn get_site_name_first_char(refno: RefU64, database: &ArDatabase) -> anyhow::Result<String> {
    let site = query_ancestor_name_of_type_aql(database, refno, "SITE").await?;
    let Some(site_name) = site else { return Ok("".to_string()); };
    if site_name.len() == 0 { return Ok("".to_string()); }
    Ok(site_name[1..2].to_string())
}

/// 获取电气设备信息
pub async fn get_dq_equi_data(refnos: Vec<RefU64>, database: &ArDatabase, project_code: &str) -> anyhow::Result<DataCenterProject> {
    let mut result = Vec::new();
    if let Ok(children) = query_refnos_travel_children_with_type_aql(database, &refnos, vec!["EQUI".to_string()]).await {
        for child in children {
            let mut attr = Vec::new();
            if !split_equi_name(&child.name, &mut attr) { continue; }
            let room_name = query_room_name_from_refno_aql(child.refno, &database).await?.unwrap_or("".to_string());
            attr.push(DataCenterAttr {
                attribute_model_code: "COMP8".to_string(),
                value: AttrValue::AttrString(room_name).into(),
            });
            // attr.push(DataCenterAttr {
            //     attribute_model_code: "COMPAA25".to_string(),
            //     value: AttrValue::AttrString("[0.0,0.0,0.0]".to_string()).into(),
            // });
            result.push(DataCenterInstance {
                object_model_code: "COMPAA".to_string(),
                project_code: project_code.to_string(),
                instance_code: child.name,
                version: get_refno_latest_version(),
                attributes: attr,
            });
        }
    }
    let project = DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: project_code.to_string(),
        owner: "KY1801".to_string(),
        instances: result,
    };
    Ok(project)
}

/// 将 name 分割 长度为 9的分割
///
/// 例如将 1VCR009VA 分割为 1  VCR  009  VA 并放到 datacenter_vec集合中
///
/// 返回 false 代表 name 长度小于9
fn split_equi_name(name: &str, mut datacenter_vec: &mut Vec<DataCenterAttr>) -> bool {
    let name = if name.starts_with("/") { name[1..].to_string() } else { name.to_string() };
    if name.len() < 8 { return false; };
    let replace_name = name.replace("-", "");
    datacenter_vec.push(DataCenterAttr {
        attribute_model_code: "COMP1".to_string(),
        value: AttrValue::AttrString(replace_name[..1].to_string()).into(),
    });
    if name.len() == 8 {
        datacenter_vec.push(DataCenterAttr {
            attribute_model_code: "COMP2".to_string(),
            value: AttrValue::AttrString(replace_name[1..3].to_string()).into(),
        });
        datacenter_vec.push(DataCenterAttr {
            attribute_model_code: "COMP3".to_string(),
            value: AttrValue::AttrString(replace_name[3..6].to_string()).into(),
        });
        datacenter_vec.push(DataCenterAttr {
            attribute_model_code: "COMP4".to_string(),
            value: AttrValue::AttrString(replace_name[6..8].to_string()).into(),
        });
    } else {
        datacenter_vec.push(DataCenterAttr {
            attribute_model_code: "COMP2".to_string(),
            value: AttrValue::AttrString(replace_name[1..4].to_string()).into(),
        });
        datacenter_vec.push(DataCenterAttr {
            attribute_model_code: "COMP3".to_string(),
            value: AttrValue::AttrString(replace_name[4..7].to_string()).into(),
        });
        datacenter_vec.push(DataCenterAttr {
            attribute_model_code: "COMP4".to_string(),
            value: AttrValue::AttrString(replace_name[7..9].to_string()).into(),
        });
    }
    datacenter_vec.push(DataCenterAttr {
        attribute_model_code: "COMP6".to_string(),
        value: name.to_string(),
    });
    true
}

#[tokio::test]
async fn test_get_machine_equi_data() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option_for_test(&db_option).await?;
    let refno = RefU64::from_str("23584/107").unwrap();
    let pool = AiosDBManager::get_db_pool(&url, "avevamarinesample").await?;
    let project = get_gy_equi_data(vec![refno], &database).await.unwrap();
    let mut file = fs::File::create("机械设备.json").unwrap();
    let data = serde_json::to_string(&project).unwrap();
    file.write_all(&data.into_bytes()).unwrap();
    Ok(())
}

#[test]
fn test_split() {
    let input = "ZZZL762";
    let first = &input[..4];
    let seconde = &input[4..];
    dbg!(&first);
    dbg!(&seconde);
}