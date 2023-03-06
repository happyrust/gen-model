use std::collections::HashMap;
use aios_core::data_center::AttrValue::{AttrIntArray, AttrMap, AttrString};
use aios_core::data_center::DataCenterAttr;
use aios_core::pdms_types::RefU64;

/// 获取 管段元数据
pub fn get_data_center_bran_attr(refno:RefU64) -> Vec<DataCenterAttr> {
    let mut result = Vec::new();
    let segma_1 = vec![1,2,3,4];
    let segma_2 = "TEST".to_string();
    let segma_3 = "安装基准点".to_string();
    let segma_4 = "TEST".to_string();
    let segma_5 = "TEST".to_string();
    let segma_6 = "TEST".to_string();
    let segma_7 = "TEST".to_string();
    let mut map = HashMap::new();
    map.insert("流向1".to_string(),vec!["支吊架编号1".to_string(),"支吊架编号2".to_string()]);
    let segma_8 = map;
    let segma_9 = "TEST".to_string();
    let segma_10 = "TEST".to_string();

    result.push(DataCenterAttr{
        attribute_model_code: "SEGMA1".to_string(),
        value: AttrIntArray(segma_1),
    });
    result.push(DataCenterAttr{
        attribute_model_code: "SEGMA2".to_string(),
        value: AttrString(segma_2),
    });
    result.push(DataCenterAttr{
        attribute_model_code: "SEGMA3".to_string(),
        value: AttrString(segma_3),
    });
    result.push(DataCenterAttr{
        attribute_model_code: "SEGMA2".to_string(),
        value: AttrString(segma_4),
    });
    result.push(DataCenterAttr{
        attribute_model_code: "SEGMA5".to_string(),
        value: AttrString(segma_5),
    });
    result.push(DataCenterAttr{
        attribute_model_code: "SEGMA6".to_string(),
        value: AttrString(segma_6),
    });
    result.push(DataCenterAttr{
        attribute_model_code: "SEGMA7".to_string(),
        value: AttrString(segma_7),
    });
    result.push(DataCenterAttr{
        attribute_model_code: "SEGMA8".to_string(),
        value: AttrMap(segma_8),
    });
    result.push(DataCenterAttr{
        attribute_model_code: "SEGMA9".to_string(),
        value: AttrString(segma_9),
    });
    result.push(DataCenterAttr{
        attribute_model_code: "SEGMA10".to_string(),
        value: AttrString(segma_10),
    });
    result
}