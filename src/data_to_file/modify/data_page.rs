use std::fs;
use std::io::{Read, Write};
use aios_core::{AttrVal, get_default_pdms_db_info};
use aios_core::pdms_types::*;
use aios_core::tool::db_tool::db1_hash;
use memchr::memmem::rfind_iter;
use crate::data_to_file::modify::modify::{convert_new_data_page, find_data_in_origin_file, ModifyNewData};
use crate::data_to_file::OldDataPage;

pub struct DataPageModify {
    pub last_page_no: u32,
    pub refno: RefU64,
    pub attr_type: String,
    pub noun_type: String,
    pub data: AttrVal,
}

impl DataPageModify {
    pub fn convert_new_data_page_modify(self, input: &[u8]) -> Option<Vec<u8>> {
        let latest_data_page = get_latest_data_page(input, self.refno, &self.attr_type.clone());
        if latest_data_page.is_none() { return None; };
        let latest_data_page = latest_data_page.unwrap();
        // 通过旧的属性页 修改对应的属性
        let modify_data_page = ModifyNewData {
            refno: self.refno,
            attr_type: self.attr_type,
            noun_type: self.noun_type,
            data: self.data,
        };
        convert_new_data_page(latest_data_page, modify_data_page, get_default_pdms_db_info(), self.last_page_no)
    }
}

/// 根据 refno + type 找到在文件中最新的数据
pub fn get_latest_data_page(input: &[u8], refno: RefU64, att_type: &str) -> Option<OldDataPage> {
    let mut refno_bytes = refno.0.to_be_bytes()[..8].to_vec();
    let mut type_hash = db1_hash(att_type).to_be_bytes()[..4].to_vec();
    refno_bytes.append(&mut type_hash);
    find_data_in_origin_file(input, &refno_bytes)
}

// #[test]
// fn test_convert_new_data_page() {
//     let mut file = fs::File::open("resource/sam7200_0001").unwrap();
//     let mut input = vec![];
//     file.read_to_end(&mut input).unwrap();
//
//     let info = serde_json::from_str::<PdmsDatabaseInfo>(&include_str!("../../../all_attr_info.json")).unwrap();
//
//     let data_page = DataPageModify {
//         last_page_no: 0xF29,
//         refno: RefU64::from_str("23584/5931").unwrap(),
//         attr_type: "STWALL".to_string(),
//         noun_type: "POS".to_string(),
//         data: AttrVal::Vec3Type([13898.39, -1534.99, 0.0]),
//         info_map: info,
//     };
//
//     let result = data_page.convert_new_data_page_modify(&input).unwrap();
//     let mut file = fs::File::create("resource/sam7200_0001_test_data").unwrap();
//     file.write_all(&result).unwrap();
// }

// #[test]
// fn test_convert_new_data_page_explicit_data() {
//     let mut file = fs::File::open("resource/sam7200_0001").unwrap();
//     let mut input = vec![];
//     file.read_to_end(&mut input).unwrap();
//
//     let info = serde_json::from_str::<PdmsDatabaseInfo>(&include_str!("../../../all_attr_info.json")).unwrap();
//
//     let data_page = DataPageModify {
//         last_page_no: 0xF29,
//         refno: RefU64::from_str("23584/5931").unwrap(),
//         attr_type: "STWALL".to_string(),
//         noun_type: "DRGP".to_string(),
//         data: AttrVal::IntegerType(100),
//         info_map: info,
//     };
//
//     let result = data_page.convert_new_data_page_modify(&input).unwrap();
//     let mut file = fs::File::create("resource/sam7200_0001_test_data").unwrap();
//     file.write_all(&result).unwrap();
// }