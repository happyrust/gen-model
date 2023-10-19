use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance, DataCenterProject};
use aios_core::pdms_types::RefU64;
use crate::aql_api::children::query_refnos_travel_children_with_type_aql;
use crate::data_center_api::data_api::{get_refno_desc, get_refno_latest_version, get_refno_paras};
use crate::data_interface::tidb_manager::AiosDBManager;

/// 获取电气螺纹杆数据
pub async fn get_dq_jldatu_fixing_data(refno: RefU64,
                                       spre_name: &str,
                                       mut fixing_attrs: &mut Vec<DataCenterAttr>,
                                       aios_mgr: &AiosDBManager) {
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PART3".to_string(),
        value: AttrValue::AttrString("螺纹杆".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD3".to_string(),
        value: AttrValue::AttrString("螺纹杆".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD4".to_string(),
        value: AttrValue::AttrString(spre_name.to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD5".to_string(),
        value: AttrValue::AttrString("".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD7".to_string(),
        value: AttrValue::AttrString("2".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD8".to_string(),
        value: AttrValue::AttrString("F-SC1".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD9".to_string(),
        value: AttrValue::AttrString("NA".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD10".to_string(),
        value: AttrValue::AttrString("抗震I级".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD11".to_string(),
        value: AttrValue::AttrString("不锈钢".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD12".to_string(),
        value: AttrValue::AttrString("个".to_string()).into(),
    });
    let desc = get_refno_desc(refno, aios_mgr).await.unwrap_or("".to_string());
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTD15".to_string(),
        value: desc,
    });
    let paras = get_refno_paras(refno, aios_mgr).unwrap_or(vec![]);
    let para_1 = paras.get(0).map_or(0.0, |x| *x);
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTDG26".to_string(),
        value: AttrValue::AttrString("M20".to_string()).into(),
    });
    fixing_attrs.push(DataCenterAttr {
        attribute_model_code: "PARTDG27".to_string(),
        value: AttrValue::AttrFloat(para_1 as f32).into(),
    });
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