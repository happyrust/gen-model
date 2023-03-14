use aios_core::data_center::{AttrValue, DataCenterAttr, DataCenterInstance};
use aios_core::pdms_types::RefU64;

/// 获得机械设备的数据
pub fn get_machine_equi_data(refno:RefU64) -> DataCenterInstance {
    let mut attr = Vec::new();
    attr.push(DataCenterAttr {
        attribute_model_code: "COMPB1".to_string(),
        value: AttrValue::AttrString("Test".to_string()),
    });
    attr.push(DataCenterAttr {
        attribute_model_code: "COMPB2".to_string(),
        value: AttrValue::AttrString("预埋板".to_string()),
    });
    attr.push(DataCenterAttr {
        attribute_model_code: "COMPB3".to_string(),
        value: AttrValue::AttrFloat(0.0),
    });
    attr.push(DataCenterAttr {
        attribute_model_code: "COMPB4".to_string(),
        value: AttrValue::AttrFloat(0.0),
    });
    attr.push(DataCenterAttr {
        attribute_model_code: "COMPB5".to_string(),
        value: AttrValue::AttrFloat(0.0),
    });
    DataCenterInstance {

        object_model_code: "1516".to_string(),
        instance_code: "KY1801-208".to_string(),
        attributes: attr,
    }
}