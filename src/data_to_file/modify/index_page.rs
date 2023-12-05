use std::fs;
use std::io::{Read, Write};
use std::str::FromStr;
use aios_core::pdms_types::RefU64;
use memchr::memmem::rfind_iter;
use crate::cata::resolve::parse_to_u64;
use crate::data_to_file::{get_latest_page, get_refno_position_in_page};

const INDEX_PAGE_ONE: [u8; 12] = [0x0u8, 0x0, 0x0, 0x5, 0x0, 0xCC, 0x47, 0xDF, 0, 0, 0, 0];
const INDEX_PAGE_TWO: [u8; 12] = [0x0u8, 0x0, 0x0, 0x5, 0x0, 0xCC, 0x47, 0xDF, 0, 0, 0, 1];
const INDEX_PAGE_THREE: [u8; 12] = [0x0u8, 0x0, 0x0, 0x5, 0x0, 0xCC, 0x47, 0xDF, 0, 0, 0, 2];

pub struct IndexPage {
    /// 修改的参考号
    pub refno: RefU64,
    /// 新增的数据中 data_page 的 page_num
    pub data_page_num: u32,
}

impl IndexPage {
    pub fn convert_new_index_page(self, input: &[u8]) -> Option<Vec<u8>> {
        // 修改 index_page_one 中 修改的参考后的后 4个byte数据
        let last_index_page_one = get_latest_page(input, self.refno, INDEX_PAGE_ONE);
        if last_index_page_one.is_none() { return None; }
        let (mut last_index_page_one, last_index_page_one_position) = last_index_page_one.unwrap();
        let refno_position = get_refno_position_in_page(&last_index_page_one, self.refno);
        if refno_position.is_none() { return None; }
        let refno_position = refno_position.unwrap();
        last_index_page_one.splice(refno_position + 8..refno_position + 12, self.data_page_num.to_be_bytes()[..4].to_vec());

        // 找到 index_page_one 数据页的第一个参考号,并返回该参考号所在的 index_page
        let b_index_page_two = input[last_index_page_one_position - 0x800..last_index_page_one_position - 0x800 + 12] == INDEX_PAGE_TWO;
        let index_page_one_first_refno = get_index_page_first_refno(&last_index_page_one);
        let index_page_two = if b_index_page_two {
            Some((input[last_index_page_one_position - 0x800..last_index_page_one_position].to_vec(), last_index_page_one_position - 0x800))
        } else {
            get_latest_page(input, index_page_one_first_refno, INDEX_PAGE_TWO)
        };
        if index_page_two.is_none() { return None; }
        let (mut index_page_two, _) = index_page_two.unwrap();
        let index_page_two_first_refno = get_index_page_first_refno(&index_page_two);
        // 修改 index_page_two 的值
        let index_page_two_refno_position = get_refno_position_in_page(&index_page_two, index_page_one_first_refno);
        if index_page_two_refno_position.is_none() { return None; }
        let index_page_two_refno_position = index_page_two_refno_position.unwrap();
        // index_page_two 该 refno 0..4个 byte 数据是 index_page_one 所在的 page_num,这里默认从 data_page 到 index_page_one 中间相隔一个page : index_page_two
        let index_page_two_page_num = self.data_page_num + 2;
        index_page_two.splice(index_page_two_refno_position + 8..index_page_two_refno_position + 12, index_page_two_page_num.to_be_bytes()[..4].to_vec());

        // 判断index_page_one是否有index_page_three
        let b_index_page_three = input[last_index_page_one_position + 0x800..last_index_page_one_position + 0x800 + 12] == INDEX_PAGE_THREE;
        // 增加 index_page_three
        if b_index_page_three {
            let mut index_page_three = get_latest_index_page_three(input).unwrap(); // 上一行已经确认过了该文件中必定有 index_page_three
            let pos = get_refno_position_in_page(&index_page_three, index_page_two_first_refno);
            if let Some(pos) = pos {
                index_page_three.splice(pos + 8..pos + 12, (self.data_page_num + 1).to_be_bytes()[..4].to_vec());
            }
            last_index_page_one.append(&mut index_page_three);
        }

        // 合并两个page
        index_page_two.append(&mut last_index_page_one);
        Some(index_page_two)
    }
}

/// 找到index_page中第一个出现的参考号
fn get_index_page_first_refno(index_page: &Vec<u8>) -> RefU64 {
    return if index_page[28..36] == [0x80, 0, 0, 1, 0x80, 0, 0, 1] {
        RefU64(parse_to_u64(&index_page[44..52]))
    } else {
        let bytes = parse_to_u64(&index_page[28..36]);
        RefU64(bytes)
    };
}

/// 找到最新的 index_page_three
fn get_latest_index_page_three(input: &[u8]) -> Option<Vec<u8>> {
    if let Some(pos) = rfind_iter(input, &INDEX_PAGE_THREE[..]).next() {
        return Some(input[pos..pos + 0x800].to_vec());
    }
    None
}

#[test]
fn test_convert_new_index_page() {
    let mut file = fs::File::open("resource/sam7200_0001").unwrap();
    let mut input = vec![];
    file.read_to_end(&mut input).unwrap();

    let data = IndexPage {
        refno: RefU64::from_str("23584/5931").unwrap(),
        data_page_num: 0xF30,
    };
    let result = data.convert_new_index_page(&input).unwrap();

    let mut file = fs::File::create("resource/sam7200_0001_test_index").unwrap();
    file.write_all(&result).unwrap();
}