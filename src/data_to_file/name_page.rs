use aios_core::pdms_types::RefU64;
use memchr::memmem::rfind_iter;
use crate::cata::resolve::{parse_to_u32, parse_to_u64};
use crate::data_to_file::{get_latest_page, get_refno_position_in_page};

const NAME_PAGE_ONE: [u8; 12] = [0, 0, 0, 5, 0, 9, 0xC1, 0x8E, 0, 0, 0, 0];
const NAME_PAGE_TWO: [u8; 12] = [0, 0, 0, 5, 0, 9, 0xC1, 0x8E, 0, 0, 0, 1];

pub struct NamePageModify {
    pub refno: RefU64,
    pub old_name: String,
    pub new_name: String,
}

impl NamePageModify {
    /// 生成新的name_page，暂时只支持已存在的 name ,且不支持中文
    pub fn convert_new_name_page(self, input: &[u8]) -> Option<Vec<u8>> {
        // 找到包含修改的参考号的name_page
        let latest_name_page = get_latest_page(input, self.refno, NAME_PAGE_ONE);
        if latest_name_page.is_none() { return None; }
        let (mut latest_name_page, _) = latest_name_page.unwrap();
        // 找到修改的参考号在latest_name_page中的位置
        let refno_position = get_refno_position_in_page(&latest_name_page, self.refno);
        if refno_position.is_none() { return None; }
        let refno_position = refno_position.unwrap(); // name_page 是 长度(不包含参考号)和 name 在前,参考号在后
        // 找到该参考号对应的name的整条数据
        let mut name_position_iter = rfind_iter(&latest_name_page, self.old_name.as_bytes());
        if name_position_iter.next().is_none() { return None; }
        let old_name_position = name_position_iter.next().unwrap();
        // 修改 name
        let name_data = self.new_name.as_bytes();
        let new_name_data = change_bytes_to_4_times(name_data.to_vec());
        let len = (new_name_data.len() as u32).to_be_bytes()[..4].to_vec();
        let refno = self.refno.0.to_be_bytes()[..8].to_vec();
        let new_data = [len, new_name_data, refno].concat();
        // 替换旧的数据
        latest_name_page.splice(old_name_position - 4..refno_position + 8, new_data);
        Some(latest_name_page)
    }
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