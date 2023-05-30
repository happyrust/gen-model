use std::collections::BTreeMap;
use aios_core::data_center::{AttrValue, DataCenterAttr};
use aios_core::data_center::AttrValue::{AttrFloat, AttrString};
use aios_core::pdms_types::{AttrMap, RefU64};
use crate::api::attr::query_explicit_attr;
use crate::aql_api::foreign_refnos::query_foreign_refno_aql;
use crate::data_center_api::data_api::{get_ispec_from_attr, get_rtext_from_attr};
use crate::data_interface::tidb_manager::AiosDBManager;

pub fn get_data_center_elbo_attr(refno: RefU64) -> Vec<DataCenterAttr> {
    let mut result = Vec::new();
    let item_1 = DataCenterAttr {
        attribute_model_code: "ITEMAD1".to_string(),
        value: AttrFloat(0.0).into(),
    };
    result.push(item_1);
    let item_2 = DataCenterAttr {
        attribute_model_code: "ITEMAD2".to_string(),
        value: AttrFloat(0.0).into(),
    };
    result.push(item_2);
    let item_3 = DataCenterAttr {
        attribute_model_code: "ITEMAD3".to_string(),
        value: AttrString("SCH5".to_string()).into(),
    };
    result.push(item_3);
    let item_4 = DataCenterAttr {
        attribute_model_code: "ITEMAD4".to_string(),
        value: AttrFloat(0.0).into(),
    };
    result.push(item_4);
    let item_5 = DataCenterAttr {
        attribute_model_code: "ITEMAD5".to_string(),
        value: AttrString("1/8".to_string()).into(),
    };
    result.push(item_5);
    let item_6 = DataCenterAttr {
        attribute_model_code: "ITEMAD6".to_string(),
        value: AttrString("CL".to_string()).into(),
    };
    result.push(item_6);
    let item_7 = DataCenterAttr {
        attribute_model_code: "ITEMAD7".to_string(),
        value: AttrString("BW".to_string()).into(),
    };
    result.push(item_7);
    result
}

/// 手动获取部分数据中台 布置专业
pub async fn get_data_center_attr_handle(attr: &AttrMap, aios_mgr: &AiosDBManager) -> anyhow::Result<BTreeMap<String, DataCenterAttr>> {
    let mut map = BTreeMap::new();
    let ispec = get_ispec_from_attr(attr, aios_mgr).await?;
    map.entry("ITEMA21".to_string()).or_insert(DataCenterAttr {
        attribute_model_code: "ITEMA21".to_string(),
        value: ispec,
    });
    let rtext = get_rtext_from_attr(attr, aios_mgr).await?;
    map.entry("ITEMA24".to_string()).or_insert(DataCenterAttr {
        attribute_model_code: "ITEMA24".to_string(),
        value: rtext,
    });
    let radius = get_elbo_radius(attr, aios_mgr).await?;
    map.entry("ITEMAD1".to_string()).or_insert(DataCenterAttr {
        attribute_model_code: "ITEMAD1".to_string(),
        value: radius,
    });
    Ok(map)
}

/// 获取 elbo 的弯曲半径 都默认为 para 2
async fn get_elbo_radius(attr: &AttrMap, aios_mgr: &AiosDBManager) -> anyhow::Result<String> {
    let Some(refno) = attr.get_refno() else { return Ok("".to_string()); };
    let database = aios_mgr.get_arangodb().await?;
    let catr = query_foreign_refno_aql(refno, &vec!["SPRE", "CATR"], database).await?;
    if let Some(catr) = catr {
        let Some((_, pool)) = aios_mgr.get_project_pool_by_refno(catr).await else { return Ok("".to_string()); };
        let catr_explicit = query_explicit_attr(catr, &pool).await?;
        if let Some(para) = catr_explicit.get_f64_vec("PARA") {
            if para.len() > 1 {
                return Ok(para[1].to_string());
            }
        }
    }
    Ok("".to_string())
}