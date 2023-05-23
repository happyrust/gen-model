use std::collections::HashMap;
use aios_core::data_center::DataCenterAttr;
use aios_core::pdms_types::{AttrMap, RefU64};
use anyhow::anyhow;
use calamine::{open_workbook, RangeDeserializerBuilder, Reader, Xlsx};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, Clone)]
struct DataCenterMetadataExcel {
    pub code: Option<String>,
    pub code_chinese_name: Option<String>,
    pub attr_code: Option<String>,
    pub attr_code_chinese_name: Option<String>,
    pub data_origin: Option<String>,
    pub function: Option<String>,
    pub att_type: Option<String>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct DataCenterMetadata {
    pub code: String,
    pub attr_code: String,
    pub function: String,
    pub att_type: String,
}

/// 读取处理后的专业元数据表单,将可以自动获取数据的条目返回
///
/// 返回值：key : att_type  value: 三维提资需要的数据
pub(crate) fn read_data_center_metadata_excel(excel_path: &str) -> anyhow::Result<HashMap<String, Vec<DataCenterMetadata>>> {
    let mut map = HashMap::new();
    let mut workbook: Xlsx<_> = open_workbook(excel_path)?;
    let range = workbook.worksheet_range("对象类属性")
        .ok_or(anyhow!("Cannot find Sheet '对象类属性'"))??;

    let mut iter = RangeDeserializerBuilder::new().from_range(&range)?;

    while let Some(result) = iter.next() {
        let v: DataCenterMetadataExcel = result?;
        if v.att_type.is_none() || v.function.is_none() { continue; }
        let att_type = v.att_type.unwrap();
        let function = v.function.unwrap();
        let Some(code) = v.code else { continue; };
        let Some(attr_code) = v.attr_code else { continue; };
        map.entry(att_type.clone()).or_insert_with(Vec::new).push(DataCenterMetadata {
            code,
            attr_code,
            function,
            att_type,
        });
    }
    Ok(map)
}

/// 根据处理后的元数据表单，将能自动获取数据的条目挑选出来，根据function字段,自动获取值
///
/// 返回值：DataCenterAttr:返回给数据中台的结构数据（部分)
pub(crate) fn auto_get_attr_from_metadata_excel(refno: RefU64, attr: &AttrMap, metadata: &Vec<DataCenterMetadata>) -> Vec<DataCenterAttr> {
    vec![]
}