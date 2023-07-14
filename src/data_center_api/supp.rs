use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance, DataCenterProject};
use aios_core::pdms_types::RefU64;
use crate::aql_api::children::query_refnos_travel_children_with_type_aql;
use crate::aql_api::pdms_room::query_room_name_from_refno_aql;
use crate::data_center_api::data_api::get_refno_desc;
use crate::data_interface::tidb_manager::AiosDBManager;

/// 获取电气支吊架信息
pub async fn get_dq_support_data(refnos: Vec<RefU64>, aios_mgr: &AiosDBManager) -> anyhow::Result<DataCenterProject> {
    let mut result = Vec::new();
    let database = aios_mgr.get_arango_db().await?;
    if let Ok(children) = query_refnos_travel_children_with_type_aql(&database, &refnos, vec!["STRU".to_string()]).await{
        for stru in children {
            let mut attr = Vec::new();
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB1".to_string(),
                value: AttrValue::AttrString(stru.name.to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB48".to_string(),
                value: AttrValue::AttrString("施工图阶段".to_string()).into(),
            });
            let room_name = query_room_name_from_refno_aql(stru.refno, &database).await?.unwrap_or("".to_string());
            attr.push(DataCenterAttr {
                attribute_model_code: "ROOM2".to_string(),
                value: AttrValue::AttrString(room_name).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB42".to_string(),
                value: AttrValue::AttrString("系统预设7%".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB43".to_string(),
                value: AttrValue::AttrString("系统预设10%".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB13".to_string(),
                value: AttrValue::AttrString("系统预设".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB14".to_string(),
                value: AttrValue::AttrString("系统预设".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB15".to_string(),
                value: AttrValue::AttrString("系统预设".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB16".to_string(),
                value: AttrValue::AttrString("系统预设".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB44".to_string(),
                value: AttrValue::AttrString("系统预设".to_string()).into(),
            });
            let desc = get_refno_desc(stru.refno,aios_mgr).await.unwrap_or("".to_string());
            let support_type = if desc.starts_with("S2") && desc.contains("FLOOR") { "支架".to_string() } else { "吊架".to_string() };
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB45".to_string(),
                value: AttrValue::AttrString(support_type).into(),
            });

            attr.push(DataCenterAttr {
                attribute_model_code: "STUCC14".to_string(),
                value: AttrValue::AttrString("Q355B".to_string()).into(),
            });
            attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB49".to_string(),
                value: AttrValue::AttrFloatArray(vec![0.0,0.0]).into(),
            });
            result.push(DataCenterInstance {
                object_model_code: "ERECAB".to_string(),
                project_code: aios_mgr.db_option.project_code.to_string(),
                instance_code: stru.name,
                version: "A版".to_string(),
                attributes: attr,
            });
        }
    }
    Ok(DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        owner: "KY1801".to_string(),
        instances: result,
    })
}