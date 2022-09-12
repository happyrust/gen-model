use aios_core::pdms_types::{AttrVal, PdmsDatabaseInfo, RefU64};

pub struct DataPage {
    pub last_page_no: u32,
    pub refno: RefU64,
    pub attr_type: String,
    pub noun_type: String,
    pub data: AttrVal,
    // pdms all_attr_info.json文件中的值
    pub info_map:PdmsDatabaseInfo,
}