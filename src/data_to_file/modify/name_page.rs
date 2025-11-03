use std::fs;
use std::io::{Read, Write};
use std::str::FromStr;
use aios_core::pdms_types::RefU64;
use memchr::memmem::{find_iter, rfind_iter};
use crate::cata::resolve::{parse_to_u32, parse_to_u64};
use crate::data_to_file::{get_latest_page, get_refno_position_in_page};

const NAME_PAGE_ONE: [u8; 12] = [0, 0, 0, 5, 0, 9, 0xC1, 0x8E, 0, 0, 0, 0];
const NAME_PAGE_TWO: [u8; 12] = [0, 0, 0, 5, 0, 9, 0xC1, 0x8E, 0, 0, 0, 1];

pub struct NamePageModify {
    pub refno: RefU64,
    pub old_name: String,
    pub new_name: String,
    pub latest_page_num: u32, // 当前最新的 page_num,包含写入新增的 page 
}

impl NamePageModify {
    /// 生成新的name_page，暂时只支持已存在的 name ,且不支持中文
    pub fn convert_new_name_page(self, input: &[u8]) -> Option<Vec<u8>> {
        // 找到包含修改的参考号的name_page
        let latest_name_page = get_latest_page(input, self.refno, NAME_PAGE_ONE);
        if latest_name_page.is_none() { return None; }
        let (mut latest_name_page, name_page_one_position) = latest_name_page.unwrap();
        let name_page_two = input[name_page_one_position + 0x800..name_page_one_position + 0x1000].to_vec();
        // 找到修改的参考号在latest_name_page中的位置
        let refno_position = get_refno_position_in_page(&latest_name_page, self.refno);
        if refno_position.is_none() { return None; }
        let refno_position = refno_position.unwrap(); // name_page 是 长度(不包含参考号)和 name 在前,参考号在后
        // 找到该参考号对应的name的整条数据
        let mut name_position_iter = rfind_iter(&latest_name_page, self.old_name.as_bytes());
        let name_position_iter = name_position_iter.next();
        if name_position_iter.is_none() { return None; }
        let old_name_position = name_position_iter.unwrap();
        // 修改 name
        let name_data = self.new_name.as_bytes();
        let new_name_data = change_bytes_to_4_times(name_data.to_vec());
        let len = ((new_name_data.len() as u32) / 4).to_be_bytes()[..4].to_vec();
        let refno = self.refno.0.to_be_bytes()[..8].to_vec();
        let new_data = [len, new_name_data, refno].concat();
        // 替换旧的数据
        latest_name_page.splice(old_name_position - 4..refno_position + 8, new_data);
        // 暂时不考虑修改了name之后，page 的数据超过了 0x800 的情况
        latest_name_page = latest_name_page[..0x800].to_vec();
        // 修改 name_page_two
        let name_page_two = change_name_page_two_data(&latest_name_page, self.latest_page_num, name_page_two);
        if name_page_two.is_none() { return None; }
        let name_page_two = name_page_two.unwrap();
        Some([latest_name_page, name_page_two].concat())
    }
}

/// 改变 name 表 对应的 name 的 page_num
fn change_name_page_two_data(name_page_one: &[u8], latest_page_num: u32, mut old_name_page_two: Vec<u8>) -> Option<Vec<u8>> {
    // 找到 name_page_one 的第一个 长度 + name
    let first_name_bytes = &name_page_one[28..40];
    let name_length = parse_to_u32(&first_name_bytes[..4]);
    // 找到 fist_name_bytes 在 name_page_two 中的位置
    let position_iter = find_iter(&old_name_page_two, first_name_bytes).next();
    if position_iter.is_none() { return None; }
    let position = position_iter.unwrap();
    // 修改他的 latest_page_num
    let old_page_num_position = position + (name_length as usize + 1) * 4;
    old_name_page_two.splice(old_page_num_position..old_page_num_position + 4, ((latest_page_num + 1) as u32).to_be_bytes());
    Some(old_name_page_two)
}

/// 将数据位数补成4的倍数，按 0 补齐
fn change_bytes_to_4_times(mut input: Vec<u8>) -> Vec<u8> {
    let r = 4 - input.len() % 4;
    if r != 4 {
        for _ in 0..r {
            input.push(0);
        }
    }
    input
}

#[test]
fn test_convert_new_name_page() {
    let mut file = fs::File::open("resource/sam7200_0001").unwrap();
    let mut input = vec![];
    file.read_to_end(&mut input).unwrap();
    let name_page = NamePageModify {
        refno: RefU64::from_str("23584/5931").unwrap(),
        old_name: "/Test/WALL".to_string(),
        new_name: "/Test/WALL/Write".to_string(),
        latest_page_num: 0,
    };
    let bytes = name_page.convert_new_name_page(&input).unwrap();
    let mut file = fs::File::create("resource/sam7200_0001_test_name").unwrap();
    file.write_all(&bytes).unwrap();
}