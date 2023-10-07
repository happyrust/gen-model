use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance, DataCenterProject};
use aios_core::pdms_types::RefU64;
use crate::aql_api::children::query_refnos_travel_children_with_type_aql;
use crate::data_center_api::data_api::{get_refno_desc, get_refno_latest_version, get_refno_paras};
use crate::data_interface::tidb_manager::AiosDBManager;

/// 获取电气螺纹杆数据
pub async fn get_dq_stru_data(refnos: Vec<RefU64>, aios_mgr: &AiosDBManager) -> anyhow::Result<DataCenterProject> {
    let mut instances = Vec::new();
    let database = aios_mgr.get_arango_db().await?;
    if let Ok(children) = query_refnos_travel_children_with_type_aql(&database, &refnos, vec!["STRU".to_string()]).await {
        for child in children {
            let mut data_center_attr = Vec::new();
            data_center_attr.push(DataCenterAttr {
                attribute_model_code: "ERECAB1".to_string(),
                value: child.name.to_string(),
            });
            data_center_attr.push(DataCenterAttr {
                attribute_model_code: "PARTDG26".to_string(),
                value: "M20".to_string(),
            });
            data_center_attr.push(DataCenterAttr {
                attribute_model_code: "PARTD11".to_string(),
                value: "不锈钢".to_string(),
            });
            instances.push(DataCenterInstance {
                object_model_code: "PARTDG".to_string(),
                project_code: aios_mgr.db_option.project_code.to_string(),
                instance_code: child.name,
                version: get_refno_latest_version(),
                attributes: data_center_attr,
            });
        }
    }
    Ok(DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        owner: "KY1801".to_string(),
        instances,
    })
}

/// 获取电气圆板数据
pub async fn get_dq_scoj_data(refnos: Vec<RefU64>, aios_mgr: &AiosDBManager) -> anyhow::Result<DataCenterProject> {
    let mut instances = Vec::new();
    let database = aios_mgr.get_arango_db().await?;
    if aios_mgr.db_option.project_code == "1516" {
        if let Ok(children) = query_refnos_travel_children_with_type_aql(&database, &refnos, vec!["SCOJ".to_string()]).await {
            for child in children {
                let mut attr = Vec::new();
                let desc = get_refno_desc(child.refno, aios_mgr).await.unwrap_or("".to_string());
                let Ok(paras) = get_refno_paras(child.refno, aios_mgr) else { continue; };
                if paras.len() < 3 { continue; };
                attr.push(DataCenterAttr {
                    attribute_model_code: "PARTD15".to_string(),
                    value: desc,
                });
                attr.push(DataCenterAttr {
                    attribute_model_code: "PARTDK1".to_string(),
                    value: format!("{}X{}", paras[0], paras[1]),
                });
                attr.push(DataCenterAttr {
                    attribute_model_code: "PARTDK2".to_string(),
                    value: AttrValue::AttrFloat(paras[2] as f32).into(),
                });
                attr.push(DataCenterAttr {
                    attribute_model_code: "PARTDK3".to_string(),
                    value: AttrValue::AttrFloatArray(vec![0.0, 0.0]).into(),
                });
                instances.push(DataCenterInstance {
                    object_model_code: "PARTDK2".to_string(),
                    project_code: aios_mgr.db_option.project_code.to_string(),
                    instance_code: child.name,
                    version: get_refno_latest_version(),
                    attributes: attr,
                });
            }
        }
    }
    Ok(DataCenterProject {
        package_code: DataCenterProject::convert_package_code(),
        project_code: aios_mgr.db_option.project_code.to_string(),
        owner: "KY1801".to_string(),
        instances,
    })
}