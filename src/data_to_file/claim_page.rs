use aios_core::pdms_types::RefU64;
use memchr::memmem::{find_iter, rfind_iter};
use crate::cata::resolve::{parse_to_u32, parse_to_u64};

const CLAIM_PAGE_ONE: [u8; 12] = [0x0u8, 0x0, 0x0, 0x5, 0x0, 0x74, 0x3F, 0x49, 0, 0, 0, 0];
const CLAIM_PAGE_TWO: [u8; 12] = [0x0u8, 0x0, 0x0, 0x5, 0x0, 0x74, 0x3F, 0x49, 0, 0, 0, 1];

/// 获得最后一个存在该参考号的 claim_page_2
fn get_last_claim_page(input: &[u8], refno: RefU64, page_no: [u8; 12]) -> Option<Vec<u8>> {
    // 从下往上找到所有的 claim_page
    let mut rfind_iter = rfind_iter(input, &page_no[..]);
    while let Some(pos) = rfind_iter.next() {
        if pos + 0x800 > input.len() { return None; }
        let claim_page = input[pos..pos + 0x800];
        // 找到存在修改的参考号所在的 claim_page
        if let Some(_refno_pos) = find_iter(&claim_page, &refno.to_be_bytes()).next() {
            return Some(input[pos..pos + 0x800].to_vec());
        }
    }
    None
}

/// 找到claim_page中第一个出现的参考号
fn get_claim_page_first_refno(claim_page: Vec<u8>) -> RefU64 {
    let bytes = parse_to_u64(&claim_page[28..36]);
    RefU64(bytes)
}