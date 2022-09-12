use std::fs;
use std::io::{Read, Write};
use aios_core::pdms_types::RefU64;
use crate::cata::resolve::parse_to_u64;
use crate::data_to_file::{get_last_page, get_page_no, get_refno_position_in_page};

const INDEX_PAGE_ONE: [u8; 12] = [0x0u8, 0x0, 0x0, 0x5, 0x0, 0xCC, 0x47, 0xDF, 0, 0, 0, 0];
const INDEX_PAGE_TWO: [u8; 12] = [0x0u8, 0x0, 0x0, 0x5, 0x0, 0xCC, 0x47, 0xDF, 0, 0, 0, 1];

pub struct IndexPage {
    /// 修改的参考号
    pub refno: RefU64,
    /// 新增的数据中 data_page 的 page_num
    pub data_page_num: u32,
}

impl IndexPage {
    pub fn convert_new_index_page(self, input: &[u8]) -> Option<Vec<u8>> {
        // 修改 index_page_one 中 修改的参考后的后 4个byte数据
        let last_index_page_one = get_last_page(input, self.refno, INDEX_PAGE_ONE);
        if last_index_page_one.is_none() { return None; }
        let mut last_index_page_one = last_index_page_one.unwrap();
        let refno_position = get_refno_position_in_page(&last_index_page_one, self.refno);
        if refno_position.is_none() { return None; }
        let refno_position = refno_position.unwrap();
        last_index_page_one.splice(refno_position + 8..refno_position + 12, self.data_page_num.to_be_bytes()[..4].to_vec());

        // 找到 index_page_one 数据页的第一个参考号,并返回该参考号所在的 index_page
        let index_page_one_first_refno = get_index_page_first_refno(&last_index_page_one);
        let index_page_two = get_last_page(input, index_page_one_first_refno, INDEX_PAGE_TWO);
        if index_page_two.is_none() { return None; }
        let mut index_page_two = index_page_two.unwrap();
        // 修改 index_page_two 的值
        let index_page_two_refno_position = get_refno_position_in_page(&index_page_two, index_page_one_first_refno);
        if index_page_two_refno_position.is_none() { return None; }
        let index_page_two_refno_position = index_page_two_refno_position.unwrap();
        // index_page_two 该 refno 0..4个 byte 数据是 index_page_one 所在的 page_num,这里默认从 data_page 到 index_page_one 中间相隔一个page : index_page_two
        let index_page_two_page_num = self.data_page_num + 2;
        index_page_two.splice(index_page_two_refno_position + 8..index_page_two_refno_position + 12, index_page_two_page_num.to_be_bytes()[..4].to_vec());

        // 合并两个page
        index_page_two.append(&mut last_index_page_one);
        Some(index_page_two)
    }
}

/// 找到index_page中第一个出现的参考号
fn get_index_page_first_refno(claim_page: &Vec<u8>) -> RefU64 {
    let bytes = parse_to_u64(&claim_page[28..36]);
    RefU64(bytes)
}

#[test]
fn test_convert_new_index_page(){
    let mut file = fs::File::open("resource/sam7200_0001").unwrap();
    let mut input = vec![];
    file.read_to_end(&mut input).unwrap();

    let data = IndexPage{
        refno: RefU64::from_refno_str("23584/5931").unwrap(),
        data_page_num: 0xF30,
    };
    let result = data.convert_new_index_page(&input).unwrap();

    let mut file = fs::File::create("resource/sam7200_0001_test_index").unwrap();
    file.write_all(&result).unwrap();
}