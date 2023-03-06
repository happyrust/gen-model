use aios_core::data_center::{AttrValue, DataCenterAttr};
use aios_core::data_center::AttrValue::{AttrFloat, AttrString};
use aios_core::pdms_types::RefU64;

pub fn get_data_center_tubi_attr(refno: RefU64) -> Vec<DataCenterAttr> {
    let mut result = Vec::new();
    let item_1 = DataCenterAttr {
        attribute_model_code: "ITEMAB1".to_string(),
        value: AttrString("SCH5".to_string()),
    };
    result.push(item_1);
    let item_2 = DataCenterAttr {
        attribute_model_code: "ITEMAB2".to_string(),
        value:AttrString("SCH5".to_string()),
    };
    result.push(item_2);
    let item_3 = DataCenterAttr {
        attribute_model_code: "ITEMAB3".to_string(),
        value:  AttrFloat(0.0),
    };
    result.push(item_3);
    let item_4 = DataCenterAttr {
        attribute_model_code: "ITEMAB4".to_string(),
        value:  AttrFloat(0.0),
    };
    result.push(item_4);
    let item_5 = DataCenterAttr {
        attribute_model_code: "ITEMAB5".to_string(),
        value: AttrString("test".to_string()),
    };
    result.push(item_5);
    let item_6 = DataCenterAttr {
        attribute_model_code: "ITEMAB6".to_string(),
        value: AttrString("test".to_string()),
    };
    result.push(item_6);
    let item_7 = DataCenterAttr {
        attribute_model_code: "ITEMAB7".to_string(),
        value: AttrString("BW".to_string()),
    };
    result.push(item_7);
    let item_8 = DataCenterAttr {
        attribute_model_code: "ITEMAB8".to_string(),
        value: AttrString("BW".to_string()),
    };
    result.push(item_8);
    let item_9 = DataCenterAttr {
        attribute_model_code: "ITEMAB9".to_string(),
        value: AttrString("1/8".to_string()),
    };
    result.push(item_9);
    let item_10 = DataCenterAttr {
        attribute_model_code: "ITEMAB10".to_string(),
        value: AttrString("1/8".to_string()),
    };
    result.push(item_10);
    result
}