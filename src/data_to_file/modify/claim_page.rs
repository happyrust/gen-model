use std::fs;
use std::io::{Read, Write};
use std::str::FromStr;
use aios_core::pdms_types::RefU64;
use memchr::memmem::{find_iter, rfind_iter};
use crate::cata::resolve::{parse_to_u32, parse_to_u64};
use crate::data_to_file::{get_latest_page, get_refno_position_in_page};

const CLAIM_PAGE_ONE: [u8; 12] = [0x0u8, 0x0, 0x0, 0x5, 0x0, 0x74, 0x3F, 0x49, 0, 0, 0, 0];
const CLAIM_PAGE_TWO: [u8; 12] = [0x0u8, 0x0, 0x0, 0x5, 0x0, 0x74, 0x3F, 0x49, 0, 0, 0, 1];

pub struct ClaimPageModify {
    /// 修改之前最后的page_number
    pub last_page_no: u32,
    /// 修改的参考号
    pub refno: RefU64,
    /// 新增内容 claim_page 中带 world 的 page_num
    pub world_claim_page_num: u32,
    /// 新增的数据中最后一个 index_page 的 page_num
    pub index_page_num:u32,
}

impl ClaimPageModify {
    /// 生成新的claim_page，需要传入pdms数据文件和修改的参考号
    pub fn convert_new_claim_page(self, input: &[u8]) -> Option<Vec<u8>> {
        // 找到修改前 存在 将要修改的refno 的claim_page
        let last_claim_page = get_latest_page(input, self.refno, CLAIM_PAGE_ONE);
        if last_claim_page.is_none() { return None; }
        let (mut last_claim_page,_) = last_claim_page.unwrap();

        // 找到该claim_page最上面的参考号
        let first_refno = get_claim_page_first_refno(&last_claim_page);
        let claim_page_two = get_latest_page(input, first_refno, CLAIM_PAGE_TWO);
        if claim_page_two.is_none() { return None;}
        let (mut claim_page_two ,_)= claim_page_two.unwrap();

        // 修改该claim_page_one中的值,暂时只修改该参考号后第 4..8 byte 的 page_num，0..4 byte 暂时不管
        let refno_position = get_refno_position_in_page(&last_claim_page,self.refno);
        if refno_position.is_none() { return None; }
        let refno_position = refno_position.unwrap();
        last_claim_page.splice(refno_position + 12 .. refno_position + 16,(self.last_page_no + 1).to_be_bytes()[..4].to_vec());

        // 修改claim_page_two中的值
        if self.world_claim_page_num != 0 {
            claim_page_two.splice(36..40, self.world_claim_page_num.to_be_bytes()[..4].to_vec());
        }
        let refno_position = get_refno_position_in_page(&claim_page_two,first_refno);
        if refno_position.is_none() { return None; }
        let refno_position = refno_position.unwrap();
        // 暂时认定first_refno后 0..4 的数据是第一个出现 新增的 claim_page 的位置，且默认claim_page在index_page之后出现
        claim_page_two.splice(refno_position + 8 .. refno_position + 12,self.index_page_num.to_be_bytes().to_vec());

        // 将 claim_page合在一起
        last_claim_page.append(&mut claim_page_two);
        Some(last_claim_page)
    }
}

/// 找到claim_page中第一个出现的参考号
fn get_claim_page_first_refno(claim_page: &Vec<u8>) -> RefU64 {
    let bytes = parse_to_u64(&claim_page[28..36]);
    RefU64(bytes)
}


#[test]
fn test_convert_new_claim_page() {
    let mut file = fs::File::open("resource/sam7200_0001").unwrap();
    let mut input = vec![];
    file.read_to_end(&mut input).unwrap();

    let data = ClaimPageModify {
        last_page_no: 0x25,
        refno: RefU64::from_str("23584/5931").unwrap(),
        world_claim_page_num: 0xF2B,
        index_page_num: 0xF33,
    };
    let result = data.convert_new_claim_page(&input).unwrap();
    let mut file = fs::File::create("resource/sam7200_0001_test_claim").unwrap();
    file.write_all(&result).unwrap();
}