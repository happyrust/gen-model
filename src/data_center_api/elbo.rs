use aios_core::data_center::{AttrValue, DataCenterAttr};
use aios_core::data_center::AttrValue::{AttrFloat, AttrString};
use aios_core::pdms_types::RefU64;

pub fn get_data_center_elbo_attr(refno: RefU64) -> Vec<DataCenterAttr> {
    let mut result = Vec::new();
    let item_1 = DataCenterAttr {
        attribute_model_code: "ITEMAD1".to_string(),
        value:  AttrFloat(0.0).into(),
    };
    result.push(item_1);
    let item_2 = DataCenterAttr {
        attribute_model_code: "ITEMAD2".to_string(),
        value:  AttrFloat(0.0).into(),
    };
    result.push(item_2);
    let item_3 = DataCenterAttr {
        attribute_model_code: "ITEMAD3".to_string(),
        value: AttrString("SCH5".to_string()).into(),
    };
    result.push(item_3);
    let item_4 = DataCenterAttr {
        attribute_model_code: "ITEMAD4".to_string(),
        value:  AttrFloat(0.0).into(),
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