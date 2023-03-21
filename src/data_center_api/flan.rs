use aios_core::data_center::{AttrValue, DataCenterAttr};
use aios_core::data_center::AttrValue::{AttrFloat, AttrStrArray, AttrString};
use aios_core::pdms_types::RefU64;

pub fn get_data_center_flan_attr(refno: RefU64) -> Vec<DataCenterAttr> {
    let mut result = Vec::new();
    let segma_1 = vec!["001".to_string(), "100".to_string(), "50".to_string(), "500".to_string()];
    let segma_2 = vec!["001".to_string(), "100".to_string(), "50".to_string(), "500".to_string()];
    let segma_3 = vec!["001".to_string(), "100".to_string(), "50".to_string(), "500".to_string()];
    let item_1 = DataCenterAttr {
        attribute_model_code: "ITEMAE1".to_string(),
        value: AttrStrArray(segma_1).into(),
    };
    result.push(item_1);
    let item_2 = DataCenterAttr {
        attribute_model_code: "ITEMAE2".to_string(),
        value: AttrStrArray(segma_2).into(),
    };
    result.push(item_2);
    let item_3 = DataCenterAttr {
        attribute_model_code: "ITEMAE3".to_string(),
        value: AttrStrArray(segma_3).into(),
    };
    result.push(item_3);
    let item_4 = DataCenterAttr {
        attribute_model_code: "ITEMAE4".to_string(),
        value: AttrString("SCH5".to_string()).into(),
    };
    result.push(item_4);
    let item_5 = DataCenterAttr {
        attribute_model_code: "ITEMAE5".to_string(),
        value: AttrFloat(0.0).into(),
    };
    result.push(item_5);
    let item_6 = DataCenterAttr {
        attribute_model_code: "ITEMAE6".to_string(),
        value: AttrString("1/8".to_string()).into(),
    };
    result.push(item_6);
    let item_7 = DataCenterAttr {
        attribute_model_code: "ITEMAE7".to_string(),
        value: AttrString("CL".to_string()).into(),
    };
    result.push(item_7);
    let item_8 = DataCenterAttr {
        attribute_model_code: "ITEMAE8".to_string(),
        value: AttrString("BW".to_string()).into(),
    };
    result.push(item_8);
    result
}