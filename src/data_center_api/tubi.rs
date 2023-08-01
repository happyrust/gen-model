use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance};
use aios_core::data_center::AttrValue::{AttrFloat, AttrString, AttrVec3};
use aios_core::pdms_types::RefU64;
use aios_core::tool::math_tool::quat_to_pdms_ori_str;
use crate::aql_api::tubi::query_tubi_from_bran;
use crate::data_center_api::data_api::get_refno_latest_version;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::ArDatabase;

pub async fn get_data_center_tubi_attr(bran_refno: RefU64, database: &ArDatabase, aios_mgr: &AiosDBManager) -> Vec<DataCenterInstance> {
    let Ok(tubis) = query_tubi_from_bran(bran_refno, database).await else { return Vec::new(); };
    let mut instances = Vec::new();
    for (idx, tubi) in tubis.into_iter().enumerate() {
        let Some(from) = RefU64::from_arangodb_refno_str(&tubi._from) else { continue; };
        let Some(to) = RefU64::from_arangodb_refno_str(&tubi._to) else { continue; };
        let mut result = Vec::new();
        result.push(DataCenterAttr {
            attribute_model_code: "ITEM1".to_string(),
            value: AttrString(from.to_refno_string()).into(),
        });
        let item_1 = DataCenterAttr {
            attribute_model_code: "ITEMA1".to_string(),
            value: AttrString(format!("TUBI {}", idx + 1)).into(),
        };
        result.push(item_1);
        let item_2 = DataCenterAttr {
            attribute_model_code: "ITEMA2".to_string(),
            value: AttrString("TUBI".to_string()).into(),
        };
        result.push(item_2);
        let item_3 = DataCenterAttr {
            attribute_model_code: "ITEMA3".to_string(),
            value: AttrString(bran_refno.to_refno_string()).into(),
        };
        result.push(item_3);
        let item_4 = DataCenterAttr {
            attribute_model_code: "ITEMA4".to_string(),
            value: AttrString("".to_string()).into(),
        };
        result.push(item_4);
        let world_position = aios_mgr.get_world_transform(from).await.unwrap_or(None).unwrap_or_default();
        let item_5 = DataCenterAttr {
            attribute_model_code: "ITEMA5".to_string(),
            value: AttrVec3(world_position.translation).into(),
        };
        result.push(item_5);
        let item_8 = DataCenterAttr {
            attribute_model_code: "ITEMA8".to_string(),
            value: AttrString(quat_to_pdms_ori_str(&world_position.rotation)).into(),
        };
        result.push(item_8);
        let item_7 = DataCenterAttr {
            attribute_model_code: "ITEMAA7".to_string(),
            value: AttrString("BW".to_string()).into(),
        };
        result.push(item_7);
        let item_8 = DataCenterAttr {
            attribute_model_code: "ITEMAA8".to_string(),
            value: AttrFloat(0.0).into(),
        };
        result.push(item_8);
        instances.push(DataCenterInstance {
            object_model_code: "ITEMAA".to_string(),
            project_code: aios_mgr.db_option.project_code.to_string(),
            instance_code: from.to_refno_string(),
            version: get_refno_latest_version(),
            attributes: result,
        });
    }
    instances
}