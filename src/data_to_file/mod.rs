use std::collections::BTreeMap;
use aios_core::{AttrMap, BHashMap};
use aios_core::helper::parse_to_u32;
use aios_core::pdms_types::*;
use aios_core::tool::db_tool::db1_hash;
use dashmap::DashMap;
use memchr::memmem::{find_iter, rfind_iter};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use crate::data_to_file::increment::data_page::{ATT_BYTES_SET, convert_new_node_data_explicit, convert_new_node_data_implicit};

pub mod modify;
pub mod increment;
mod create_att;


const INDEX_PAGE: [u8; 8] = [0x0, 0x0, 0x0, 0x5, 0x0, 0xCC, 0x47, 0xDF];
const CLAIM_PAGE: [u8; 8] = [0x0u8, 0x0, 0x0, 0x5, 0x0, 0x74, 0x3F, 0x49];

pub enum PageType {
    // 数据页
    DataPage,
    // 索引页
    IndexPage,
    ClaimPage,
    SessionPage,
}

/// 修改属性后，该文件的所有数据页
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct NewPage {
    /// 原始文件
    pub origin_file: Vec<u8>,
    /// 属性页
    pub data_page: Vec<u8>,
    /// 版本号页 1
    pub first_version_page: Vec<u8>,
    /// 版本号页 2
    pub second_version_page: Vec<u8>,
    /// 修改次数页
    pub change_times_page: Vec<u8>,
    /// 会话页
    pub conversion_page: Vec<u8>,
}

impl NewPage {
    pub fn convert_into_one_page(self) -> Vec<u8> {
        [self.origin_file, self.data_page, self.first_version_page,
            self.second_version_page, self.change_times_page,
            self.conversion_page
        ].concat()
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct OldDataPage {
    /// 隐式属性
    pub implicit_data: Vec<u8>,
    /// 子节点
    pub children: Vec<u8>,
    /// 显示属性
    pub explicit_data: Vec<u8>,
}

impl OldDataPage {
    pub fn turn_self_into_vec(self) -> Vec<u8> {
        [self.implicit_data, self.children, self.explicit_data].concat()
    }

    pub fn convert_new_data_page(self) -> Vec<u8> {
        let mut result = vec![0; 0x800];
        let value = [self.implicit_data, self.children, self.explicit_data].concat();
        result.splice(0..value.len(), value);
        result
    }

    /// 将 attr_map 还原成 bytes，暂时不考虑有children的情况
    pub fn from_attr_map(refno:RefU64,attr_map: AttrMap, info_map: DashMap<i32, DashMap<i32, AttrInfo>>) -> Option<Self> {
        let mut implicit_map = BTreeMap::new(); // 隐式属性需要根据offset排序
        let mut explicit_map = AttrMap::default();
        let att_type = attr_map.get_type();
        // 将attr_map 分为显示和隐式两部分
        let noun_info_map = info_map.get(&(db1_hash(att_type) as i32));
        if noun_info_map.is_none() { return None; }
        let noun_info_map = noun_info_map.unwrap();
        for (noun_hash, info) in noun_info_map.clone().into_iter() {
            let noun_hash = (noun_hash as u32);
            if ATT_BYTES_SET.contains(&noun_hash) { continue; }
            if let Some(val) = attr_map.map.get(&noun_hash) {
                if info.offset == 0 {
                    explicit_map.insert(noun_hash, val.clone());
                } else {
                    implicit_map.insert(info.offset, (noun_hash, val.clone()));
                }
            } else {
                // 隐式属性为空时，保存默认值，显示属性不管
                if info.offset != 0 {
                    implicit_map.insert(info.offset, (noun_hash, info.default_val.clone()));
                }
            }
        }
        // 将分类好的属性转换成bytes
        let implicit_bytes = convert_new_node_data_implicit(implicit_map, false);
        let explicit_bytes = convert_new_node_data_explicit(refno,explicit_map, false);
        // 给显示属性加上 01和长度，隐式属性前面还有值需要添加，所有不加长度
        let mut explicit_len = vec![0u8, 1];
        explicit_len = [explicit_len, ((explicit_bytes.len() / 4 + 1) as u16).to_be_bytes().to_vec()].concat();
        Some(Self {
            implicit_data: implicit_bytes,
            children: vec![],
            explicit_data: [explicit_len, explicit_bytes].concat(),
        })
    }
}

/// 从pdms文件中，获得 pdms 最新的 page_no
pub fn get_last_page_no(input: &[u8]) -> u32 {
    parse_to_u32(&input[40..44])
}

/// 指定page的类型，通过该类型最后出现的位置,返回该类型的page_num
pub fn get_page_no(input: &[u8], page_type: PageType) -> Option<u32> {
    let index = match page_type {
        PageType::IndexPage => {
            INDEX_PAGE.to_vec()
        }
        PageType::ClaimPage => {
            CLAIM_PAGE.to_vec()
        }
        _ => { return None; }
    };
    if let Some(position) = rfind_iter(input, &index).next() {
        return Some((position / 0x800) as u32);
    }
    None
}

/// 获得最后一个存在该参考号的 page,并返回他开始的position
pub fn get_latest_page(input: &[u8], refno: RefU64, page_no: [u8; 12]) -> Option<(Vec<u8>, usize)> {
    // 从下往上找到所有的 page
    let mut rfind_iter = rfind_iter(input, &page_no[..]);
    while let Some(pos) = rfind_iter.next() {
        if pos + 0x800 > input.len() { return None; }
        let claim_page = &input[pos..pos + 0x800];
        // 找到存在修改的参考号所在的 claim_page
        if let Some(_refno_pos) = find_iter(&claim_page, &refno.to_be_bytes()).next() {
            return Some((input[pos..pos + 0x800].to_vec(), pos));
        }
    }
    None
}

/// 返回参考号在该数据页中的位置
pub fn get_refno_position_in_page(input: &[u8], refno: RefU64) -> Option<usize> {
    return if let Some(position) = find_iter(input, &refno.0.to_be_bytes()).next() {
        Some(position)
    } else {
        None
    };
}
