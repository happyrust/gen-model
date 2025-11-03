use crate::api::children::query_owner_till_type;
use crate::api::element::query_owner_from_id;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::data_to_file::modify::claim_page::ClaimPageModify;
use crate::data_to_file::modify::data_page::DataPageModify;
use crate::data_to_file::modify::index_page::IndexPage;
use crate::data_to_file::modify::name_page::NamePageModify;
use crate::data_to_file::modify::session_page::{get_latest_session_page, SessionPageModify};
use crate::data_to_file::{get_latest_page, NewPage, OldDataPage};
use aios_core::get_default_pdms_db_info;
use aios_core::helper::{parse_to_i32, parse_to_u16, parse_to_u32};
use aios_core::pdms_types::DbAttributeType::Vec3Type;
use aios_core::pdms_types::*;
use aios_core::tool::db_tool::{db1_hash, read_attr_info_config_from_json};
use aios_core::{AttrVal, PdmsDatabaseInfo};
use bitvec::field::BitField;
use bitvec::prelude::Lsb0;
use bitvec::view::BitView;
use dashmap::DashMap;
use lazy_static::lazy_static;
use memchr::memmem::{find_iter, rfind_iter};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use sqlx::{MySql, Pool};
use std::fs::File;
use std::io::{Read, Write};
use std::mem::take;
use std::str::FromStr;
use std::{env, fs};

const FIRST_VERSION_PAGE: [u8; 20] = [
    0x0u8, 0x0, 0x0, 0x5, 0x0, 0xCC, 0x47, 0xDF, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x2, 0x0, 0x0,
    0x0, 0x2,
];
const SECOND_VERSION_PAGE: [u8; 20] = [
    0x0u8, 0x0, 0x0, 0x5, 0x0, 0xCC, 0x47, 0xDF, 0x0, 0x0, 0x0, 0x1, 0x0, 0x0, 0x0, 0x2, 0x0, 0x0,
    0x0, 0x2,
];
const FIRST_CHANGE_TIMES_PAGE: [u8; 20] = [
    0x0u8, 0x0, 0x0, 0x5, 0x0, 0x74, 0x3F, 0x49, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x2, 0x0, 0x0,
    0x0, 0x2,
];
const SECOND_CHANGE_TIMES_PAGE: [u8; 20] = [
    0x0u8, 0x0, 0x0, 0x5, 0x0, 0x74, 0x3F, 0x49, 0x0, 0x0, 0x0, 0x1, 0x0, 0x0, 0x0, 0x2, 0x0, 0x0,
    0x0, 0x2,
];
const CONVERSION_PAGE: [u8; 12] = [
    0x0u8, 0x0, 0x0, 0x2, 0x61, 0x64, 0x6D, 0x69, 0x6E, 0x0, 0x0, 0x0,
];

lazy_static! {
    // 用于写入中 修改次数页
    pub static ref PACKAGE_TYPE_MAP: DashMap<&'static str, i32> = {
        let mut map =  DashMap::new();
        map.insert("CATE",0x9D572i32);
        map.insert("CATA",0x8A1E6i32);
        map.insert("SECT",0xE26D2i32);
        map.insert("WORL",0xBEB83i32);
        map.insert("SPWL",0xBF9D7i32);
        map.insert("PRTWLD",0x3DC5838i32);
        map.insert("TABGRO",0xD70673Ei32);
        map.insert("CTABLE",0x4B0C612i32);
        map.insert("PRTELE",0x4B1E2ADi32);
        map
    };

    pub static ref PACKAGE_TYPE_VEC: Vec<String> = {
        vec!["CATE".to_string(),"CATA".to_string(),"SECT".to_string(),"WORL".to_string(),
            "SPWL".to_string(),"PRTWLD".to_string(),"TABGRO".to_string(),"CTABLE".to_string(),"PRTELE".to_string()]
    };
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ModifyNewData {
    pub refno: RefU64,
    pub attr_type: String,
    pub noun_type: String,
    pub data: AttrVal,
}

impl ModifyNewData {
    pub fn get_refno_bytes(&self) -> Vec<u8> {
        self.refno.0.to_be_bytes().to_vec()
    }

    pub fn get_refno_and_type_bytes(&self) -> Vec<u8> {
        let mut refno = self.get_refno_bytes();
        let mut attr_type = db1_hash(&self.attr_type).to_be_bytes()[..4].to_vec();
        refno.append(&mut attr_type);
        refno
    }

    pub fn get_type_hash_u32(&self) -> u32 {
        db1_hash(&self.attr_type)
    }

    pub fn get_noun_hash_u32(&self) -> u32 {
        db1_hash(&self.noun_type)
    }

    pub fn get_noun_hash_vec(&self) -> Vec<u8> {
        db1_hash(&self.noun_type).to_be_bytes()[..4].to_vec()
    }

    /// 将显示属性转换成pdms格式
    pub(crate) fn convert_explicit_data_to_bytes(
        mut noun_hash: Vec<u8>,
        mut type_len: Vec<u8>,
        len: Option<Vec<u8>>,
        mut data: Vec<u8>,
    ) -> Vec<u8> {
        noun_hash.append(&mut type_len);
        if let Some(mut len) = len {
            noun_hash.append(&mut len);
        }
        // 将数据位数补成4的倍数
        let r = 4 - data.len() % 4;
        noun_hash.append(&mut data);
        if r != 4 {
            for _ in 0..r {
                noun_hash.push(0);
            }
        }
        noun_hash
    }

    fn convert_implicit_data_to_bytes(len: Option<Vec<u8>>, data: Vec<u8>) -> Vec<u8> {
        if let Some(len) = len {
            [len, data].concat()
        } else {
            data
        }
    }

    pub fn convert_implicit_data_to_vec(&self, b_f64: bool) -> Vec<u8> {
        match self.data.clone() {
            AttrVal::Vec3Type(values) => {
                let mut value = vec![];
                if b_f64 {
                    for v in values {
                        if let [a, b, c, d, e, f, g, h] = v.to_be_bytes() {
                            value.push(vec![e, f, g, h, a, b, c, d]);
                        }
                    }
                } else {
                    for v in values {
                        value.push(v.to_be_bytes().to_vec());
                    }
                }
                let value = value.into_iter().flatten().collect::<Vec<u8>>();
                ModifyNewData::convert_implicit_data_to_bytes(Some(vec![0, 0, 0, 3]), value)
            }
            AttrVal::IntArrayType(values) => {
                let mut value = vec![];
                for v in values {
                    value.push(v.to_be_bytes().to_vec());
                }
                let len = (value.len() as u32).to_be_bytes().to_vec();
                let value = value.into_iter().flatten().collect::<Vec<u8>>();
                ModifyNewData::convert_implicit_data_to_bytes(Some(len), value)
            }
            AttrVal::WordType(v) => {
                let value = db1_hash(v.as_str()).to_be_bytes().to_vec();
                ModifyNewData::convert_implicit_data_to_bytes(None, value)
            }
            AttrVal::RefU64Type(v) => {
                let value = v.to_be_bytes().to_vec();
                ModifyNewData::convert_implicit_data_to_bytes(None, value)
            }

            _ => {
                vec![]
            }
        }
    }

    pub fn convert_explicit_data_to_vec(&self, b_f64: bool) -> Vec<u8> {
        let mut noun_hash = self.get_noun_hash_vec();
        match self.data.clone() {
            AttrVal::IntegerType(v) => ModifyNewData::convert_explicit_data_to_bytes(
                noun_hash,
                vec![0xC, 0, 0, 1],
                None,
                v.to_be_bytes()[..4].to_vec(),
            ),
            AttrVal::WordType(v) => {
                let v = db1_hash(v.as_str()).to_be_bytes().to_vec();
                ModifyNewData::convert_explicit_data_to_bytes(
                    noun_hash,
                    vec![0xC, 0, 0, 1],
                    None,
                    v,
                )
            }
            AttrVal::StringType(v) => {
                let v = v.as_bytes();
                let len = v.len() as f32;
                let mut l = [
                    vec![0x3C, 0],
                    (((len / 4.0).ceil() + 1.0) as u16).to_be_bytes().to_vec(),
                ]
                .concat();
                let len = (len as u32).to_be_bytes().to_vec();
                ModifyNewData::convert_explicit_data_to_bytes(noun_hash, l, Some(len), v.to_vec())
            }
            AttrVal::BoolType(v) => {
                if v {
                    ModifyNewData::convert_explicit_data_to_bytes(
                        noun_hash,
                        vec![0x14, 0, 0, 1],
                        None,
                        vec![0, 0, 0, 1],
                    )
                } else {
                    ModifyNewData::convert_explicit_data_to_bytes(
                        noun_hash,
                        vec![0x14, 0, 0, 1],
                        None,
                        vec![0, 0, 0, 0],
                    )
                }
            }
            AttrVal::DoubleType(v) => {
                if let [a, b, c, d, e, f, g, h] = v.to_be_bytes() {
                    let value = vec![e, f, g, h, a, b, c, d];
                    ModifyNewData::convert_explicit_data_to_bytes(
                        noun_hash,
                        vec![8, 0, 0, 2],
                        None,
                        value,
                    )
                } else {
                    vec![]
                }
            }
            AttrVal::DoubleArrayType(values) => {
                let mut value = vec![];
                let mut l = vec![];
                if b_f64 {
                    for v in values {
                        if let [a, b, c, d, e, f, g, h] = v.to_be_bytes() {
                            value.push(vec![e, f, g, h, a, b, c, d]);
                        }
                    }
                    l = ((value.len() * 2 + 1) as u16).to_be_bytes()[..2].to_vec();
                } else {
                    for v in values {
                        value.push((v as f32).to_be_bytes().to_vec());
                    }
                    l = ((value.len() + 1) as u16).to_be_bytes()[..2].to_vec();
                }

                l = [vec![0x18, 0], l].concat();
                let len = (value.len() as u32).to_be_bytes()[..4].to_vec();
                let value = value.into_iter().flatten().collect();
                ModifyNewData::convert_explicit_data_to_bytes(noun_hash, l, Some(len), value)
            }
            AttrVal::Vec3Type(values) => {
                let mut value = vec![];
                let mut l = vec![];
                if b_f64 {
                    for v in values {
                        if let [a, b, c, d, e, f, g, h] = v.to_be_bytes() {
                            value.push(vec![e, f, g, h, a, b, c, d]);
                        }
                    }
                    l = ((value.len() * 2 + 1) as u16).to_be_bytes()[..2].to_vec();
                } else {
                    for v in values {
                        value.push((v as f32).to_be_bytes().to_vec());
                    }
                    l = ((value.len() + 1) as u16).to_be_bytes()[..2].to_vec();
                }

                l = [vec![0x18, 0], l].concat();
                let value = value.into_iter().flatten().collect();
                ModifyNewData::convert_explicit_data_to_bytes(
                    noun_hash,
                    l,
                    Some(vec![0, 0, 0, 3]),
                    value,
                )
            }
            AttrVal::IntArrayType(values) => {
                let mut value = vec![];
                for v in values {
                    value.push(v.to_be_bytes().to_vec());
                }
                let len = (value.len() as u32).to_be_bytes().to_vec();
                let l = [
                    vec![0x0, 0],
                    ((value.len() + 1) as u16).to_be_bytes()[..2].to_vec(),
                ]
                .concat(); // 还没找到pdms文件中的IntArrayType数据
                let value = value.into_iter().flatten().collect::<Vec<u8>>();
                ModifyNewData::convert_explicit_data_to_bytes(noun_hash, l, Some(len), value)
            }

            _ => {
                vec![]
            }
        }
    }
}

fn modify_bool_implicit_data(input: &[u8], offset: u32, value: bool) -> u32 {
    let val_off = offset & 0xFFFFF;
    let index = (val_off >> 0x14) as usize;
    let pos = (val_off * 4) as usize;
    let mut val = parse_to_u32(&input[pos..pos + 4]);
    let mut bits = val.view_bits_mut::<Lsb0>();
    bits.set(index, value);
    bits.load_be()
}

/// 读取原文件，返回新的版本号和原数据
pub fn change_origin_file(path: &str) -> (u32, Vec<u8>) {
    let mut file = fs::File::open(path).unwrap();
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok();
    let old_version = parse_to_u32(&buf[40..44]);
    let new_version_u32 = old_version + 6;
    let new_version = (old_version + 6).to_be_bytes()[..4].to_vec();
    buf.splice(40..44, new_version);
    (new_version_u32, buf)
}

/// 传入 refno + type 返回该数据在pdms文件中的位置
pub fn find_data_in_origin_file(input: &[u8], buf: &[u8]) -> Option<OldDataPage> {
    if let Some(pos) = rfind_iter(&input, buf).next() {
        let implicit_data_len =
            (u32::from_be_bytes(input[pos - 4..pos].try_into().unwrap()) * 4 - 4) as usize;
        let implicit_data = [
            vec![0x0, 0x0, 0x0, 0x7],
            input[pos - 4..pos + implicit_data_len].to_vec(),
        ]
        .concat();
        let (children_data, explicit_data) =
            get_origin_children_and_explicit_data(&input, pos + implicit_data_len);
        return Some(OldDataPage {
            implicit_data,
            children: children_data,
            explicit_data,
        });
    }
    None
}

/// 将修改的值写入到 DataPage中
pub fn convert_new_data_page(
    mut page: OldDataPage,
    data: ModifyNewData,
    pdms_database_info: &PdmsDatabaseInfo,
    latest_page_no: u32,
) -> Option<Vec<u8>> {
    // let mut new_data = vec![];
    // let attr_type = data.get_type_hash_u32();
    // let noun = data.get_noun_hash_u32();
    // // 检测修改的属性是否是该类型存在的属性
    // let b_type_value = check_b_type_value(&pdms_database_info.noun_attr_info_map, attr_type as i32, noun as i32);
    // if !b_type_value { return None; }
    // // 修改隐式属性中的 page_no
    // let new_page_no = latest_page_no + 1;
    // page.implicit_data.splice(0x1C..0x20, new_page_no.to_be_bytes()[..4].to_vec());
    // page.implicit_data.splice(0x24..0x28, new_page_no.to_be_bytes()[..4].to_vec());
    // // 修改的内容为隐式属性
    // if let Some((noun_pos, offset)) = check_b_implicit_data(&pdms_database_info.noun_attr_info_map, attr_type as i32, noun as i32) {
    //     return match data.data {
    //         BoolType(value) => {
    //             let r = modify_bool_implicit_data(&page.implicit_data, offset, value);
    //             let position = ((offset + 1) * 4) as usize;
    //             page.implicit_data.splice(position..position + 4, r.to_be_bytes()[..4].to_vec());
    //             Some(OldDataPage {
    //                 implicit_data: page.implicit_data,
    //                 children: page.children,
    //                 explicit_data: page.explicit_data,
    //             }.convert_new_data_page())
    //         }
    //         _ => {
    //             let data = data.convert_implicit_data_to_vec(true);
    //             let len = data.len();
    //             let origin_len = page.implicit_data.len();
    //             new_data = page.implicit_data[..noun_pos + 4].to_vec();
    //             new_data = [new_data, data].concat();
    //             if noun_pos + len < origin_len {
    //                 new_data = [new_data, page.implicit_data[len + noun_pos + 4..].to_vec()].concat(); // +4是因为 data前面还有个 007
    //             }
    //             Some(OldDataPage {
    //                 implicit_data: new_data,
    //                 children: page.children,
    //                 explicit_data: page.explicit_data,
    //             }.convert_new_data_page())
    //         }
    //     };
    //     // 修改的内容为显示属性
    // } else {
    //     // 如果已存在该显示属性，则在原来的基础上修改
    //     if let Some(pos) = find_iter(&page.explicit_data, &noun.to_be_bytes()[..]).next() {
    //         let data = data.convert_explicit_data_to_vec(true);
    //         // 未修改的属性直接复制到new_data中
    //         new_data = page.explicit_data[..pos].to_vec();
    //         new_data = [new_data, data].concat();
    //         let attr_len = (u16::from_be_bytes(page.explicit_data[pos + 6..pos + 8].try_into().unwrap()) * 4) as usize;
    //         // 修改的属性后面还有未改变的值，也直接复制过来
    //         if pos + 8 + attr_len < *&page.explicit_data.len() {
    //             new_data = [new_data, page.explicit_data[pos + 8 + attr_len..].to_vec()].concat();
    //         }
    //     } else {
    //         // 若不存在则在后面新增
    //         let mut data = data.convert_explicit_data_to_vec(true);
    //         new_data = [page.explicit_data, take(&mut data)].concat();
    //     }
    //
    //
    //     let len = (*&new_data.len() as u16 / 4).to_be_bytes();
    //     new_data.splice(2..4, len); // 修改显示属性 01 后的长度
    //
    //     Some(OldDataPage {
    //         implicit_data: page.implicit_data,
    //         children: page.children,
    //         explicit_data: new_data,
    //     }.convert_new_data_page())
    // }
    None
}

#[inline]
fn check_b_implicit_data(
    map: &DashMap<i32, DashMap<i32, AttrInfo>>,
    attr_type: i32,
    noun_hash: i32,
) -> Option<(usize, u32)> {
    if let Some(info_map) = map.get(&attr_type) {
        if let Some(info) = info_map.get(&noun_hash) {
            if info.offset != 0 {
                return Some(((info.offset as usize) * 4, info.offset));
            }
        }
    }
    None
}

/// 检测该type中是否存在某属性
fn check_b_type_value(
    map: &DashMap<i32, DashMap<i32, AttrInfo>>,
    attr_type: i32,
    noun_hash: i32,
) -> bool {
    if let Some(info_map) = map.get(&attr_type) {
        return info_map.get(&noun_hash).is_some();
    }
    false
}

/// 获取该节点的 children 或者 显示属性
pub fn get_origin_children_and_explicit_data(input: &[u8], mut pos: usize) -> (Vec<u8>, Vec<u8>) {
    let mut children_data = vec![];
    let mut explicit_data = vec![];

    if &input[pos..pos + 2] == &[0x0, 0x2] {
        let data_len = (parse_to_u16(&input[pos + 2..pos + 4]) * 4) as usize;
        children_data = input[pos..pos + data_len].to_vec();
        pos = pos + data_len;
    }

    if &input[pos..pos + 2] == &[0x0, 0x1] {
        let data_len = (parse_to_u16(&input[pos + 2..pos + 4]) * 4) as usize;
        explicit_data = input[pos..pos + data_len].to_vec();
    }
    (children_data, explicit_data)
}

/// 修改第一个refno + version page 的版本号
pub fn convert_first_version_page(input: &[u8], refno: &[u8], version: u32) -> Option<Vec<u8>> {
    let version_start = &FIRST_VERSION_PAGE;

    let mut iter = rfind_iter(input, version_start);
    while let Some(pos) = iter.next() {
        let mut version_page = vec![0u8; 0x800];
        let mut version_data = input[pos..pos + 0x800].to_vec();
        if let Some(r_pos) = find_iter(&input[pos..pos + 0x800], refno).next() {
            let new_version = (version - 4).to_be_bytes()[..4].to_vec(); // 在大版本 +5 的基础上 -4
            version_data.splice(r_pos + 8..r_pos + 8 + 4, new_version);
            version_page.splice(0..0x800, version_data);
            return Some(version_page);
        }
    }
    None
}

/// 修改次数页
pub async fn convert_change_times_page(
    input: &[u8],
    refno: RefU64,
    pool: &Pool<MySql>,
) -> anyhow::Result<Option<(Vec<u8>, Vec<u8>)>> {
    let start = &FIRST_CHANGE_TIMES_PAGE;
    // 找到修改次数页，需要修改的是该参考号的owner
    if let Ok(Some(owner)) = query_owner_from_id(refno, &pool).await {
        // 找到该参考号对应的修改次数页
        let mut page_iter = rfind_iter(input, start);
        while let Some(pos) = page_iter.next() {
            let owner_bytes = &owner.0.to_be_bytes()[..8];
            let mut change_times_page = input[pos..pos + 0x800].to_vec();
            if let Some(ref_pos) = find_iter(&change_times_page, owner_bytes).next() {
                let change_times = &(parse_to_i32(&change_times_page[ref_pos + 12..ref_pos + 16])
                    + 1)
                .to_be_bytes()[..4];
                change_times_page.splice(ref_pos + 12..ref_pos + 16, change_times.to_vec());
                // 如果修改次数页有第二页，也需要加上
                // todo 修改次数页第二页 变化的参考号和本参考号看起来毫无相关
                if input[pos + 0x800..pos + 0x800 + 20] == SECOND_CHANGE_TIMES_PAGE {
                    change_times_page.append(&mut input[pos + 0x800..pos + 0x1000].to_vec());
                }
                return Ok(Some((change_times_page, change_times.to_vec())));
            }
        }
    }
    Ok(None)
}

/// 会话页
pub fn convert_conversation_page(
    input: &[u8],
    version: u32,
    change_times: &[u8],
) -> Option<Vec<u8>> {
    let v = &CONVERSION_PAGE;

    let mut new_version = version.to_be_bytes()[..4].to_vec();
    let mut new_version_reduce_2 = (version - 2).to_be_bytes()[..4].to_vec();
    let mut new_version_reduce_1 = (version - 1).to_be_bytes()[..4].to_vec();
    let mut old_version = (version - 6).to_be_bytes()[..4].to_vec();

    if let Some(pos) = rfind_iter(input, &v).next() {
        let mut old = vec![0, 0, 0, 3];
        old.append(&mut old_version);
        let mut times = vec![0, 0, 0, 1];
        times.append(&mut change_times.to_vec());
        let mut new = vec![0xFF, 0xFF, 0xFF, 0xFF];
        new.append(&mut new_version);
        let mut new_2 = vec![0, 0, 0, 1];
        new_2.append(&mut new_version_reduce_2);
        let mut new_1 = vec![0, 0, 0, 1];
        new_1.append(&mut new_version_reduce_1);

        let mut remain_data = input[pos - 80..pos - 80 + 0x7D8].to_vec();
        let l = remain_data.len();
        let new_page = (parse_to_i32(&remain_data[l - 16..l - 12]) + 1).to_be_bytes()[..4].to_vec();
        remain_data.splice(l - 16..l - 12, new_page);

        return Some([old, times, new, new_2, new_1, remain_data].concat());
    }
    None
}

pub struct ModifyData {
    // 修改之前源 pdms 文件
    pub old_file: Vec<u8>,
    pub refno: RefU64,
    pub attr_type: String,
    pub noun_type: String,
    pub data: AttrVal,
    pub old_data: AttrVal,
    /// 用户名
    pub user_name: String,
    /// 提交说明
    pub commit_comment: String,
}

/// 存放全局的 name_page type_page等
#[derive(Debug, Serialize, Deserialize)]
pub enum GlobalPage {
    // 0x9C18E
    NamePage(u32),
    // 0xCC6B3F
    TypePage(u32),
    None,
}

impl ModifyData {
    pub fn convert_new_modify_data(self) -> Option<Vec<u8>> {
        // 读取info文件
        // 获得最新的session_page_num
        let latest_session_page_num = parse_to_u32(&self.old_file[40..44]);
        // 获得最新的page_num
        let latest_session_page = get_latest_session_page(&self.old_file, latest_session_page_num);
        let latest_page_num = parse_to_u32(&latest_session_page[20..24]);
        let session_no = parse_to_u32(&latest_session_page[12..16]);
        let mut current_page_num = latest_page_num;

        // 生成属性页
        let data_page = DataPageModify {
            last_page_no: latest_page_num,
            refno: self.refno,
            attr_type: self.attr_type.clone(),
            noun_type: self.noun_type.clone(),
            data: self.data.clone(),
        };
        let data_page = data_page.convert_new_data_page_modify(&self.old_file);
        if data_page.is_none() {
            return None;
        }
        current_page_num += 1;

        // 生成 index_page
        let index_page = IndexPage {
            refno: self.refno,
            data_page_num: current_page_num,
        };
        let index_page = index_page.convert_new_index_page(&self.old_file);
        if index_page.is_none() {
            return None;
        }
        let index_page = index_page.unwrap();
        current_page_num += (index_page.len() / 0x800) as u32; // index_page 是两页或三页
        let current_index_page = current_page_num;
        // 检测是否需要修改全局信息的 page ,如果需要则添加该部分数据
        let mut global_page = vec![];
        let b_change_global_page = self.check_b_global_page(current_page_num + 1);
        match b_change_global_page {
            GlobalPage::NamePage(_) => {
                let name_page = NamePageModify {
                    refno: self.refno,
                    old_name: self.old_data.string_value(),
                    new_name: self.data.string_value(),
                    latest_page_num: current_page_num,
                };
                let new_name_bytes = name_page.convert_new_name_page(&self.old_file);
                if let Some(new_name_bytes) = new_name_bytes {
                    global_page = new_name_bytes;
                    current_page_num += 2; // name_page 是两页
                }
            }
            _ => {}
        }

        // 生成 claim_page
        let claim_page = ClaimPageModify {
            last_page_no: session_no,
            refno: self.refno,
            world_claim_page_num: 0,
            index_page_num: current_page_num + 1,
        };
        let claim_page = claim_page.convert_new_claim_page(&self.old_file);
        if claim_page.is_none() {
            return None;
        }
        let claim_page = claim_page.unwrap();
        current_page_num += (claim_page.len() / 0x800) as u32;
        let current_claim_page = current_page_num;

        // 生成 session_page
        let current_session_page = current_page_num;
        let session_page = SessionPageModify {
            last_page_num: latest_session_page_num,
            new_latest_page_num: current_page_num,
            index_page_num: current_index_page,
            global_page_num: b_change_global_page,
            claim_page_num: current_claim_page,
            user_name: self.user_name,
            commit_comment: self.commit_comment,
        };
        let session_page = session_page.convert_session_page(&self.old_file);
        // 修改文件头上的page_num
        let mut new_file = self.old_file;
        new_file.splice(40..44, (current_page_num + 1).to_be_bytes()[..4].to_vec());
        Some(
            [
                new_file,
                data_page.unwrap(),
                index_page,
                global_page,
                claim_page,
                session_page,
            ]
            .concat(),
        )
    }

    fn check_b_global_page(&self, current_page_num: u32) -> GlobalPage {
        match self.noun_type.as_str() {
            "NAME" => GlobalPage::NamePage(current_page_num),
            _ => GlobalPage::None,
        }
    }
}

#[test]
fn test_convert_new_modify_data() {
    let mut file = fs::File::open("resource/sam7200_0001").unwrap();
    let mut input = vec![];
    file.read_to_end(&mut input).unwrap();

    let modify_data = ModifyData {
        old_file: input,
        refno: RefU64::from_str("23584/5931").unwrap(),
        attr_type: "STWALL".to_string(),
        noun_type: "POSS".to_string(),
        data: AttrVal::Vec3Type([13898.39, -1534.99, 0.0]),
        old_data: Default::default(),
        user_name: "admin".to_string(),
        commit_comment: "Default session comment".to_string(),
    };
    let data = modify_data.convert_new_modify_data().unwrap();

    let mut file = fs::File::create("resource/sam7200_0001_test").unwrap();
    file.write_all(&data).unwrap();
}

#[test]
fn test_convert_new_modify_data_explict_data() {
    let mut file = fs::File::open("resource/sam7200_0001").unwrap();
    let mut input = vec![];
    file.read_to_end(&mut input).unwrap();

    let modify_data = ModifyData {
        old_file: input,
        refno: RefU64::from_str("23584/5931").unwrap(),
        attr_type: "STWALL".to_string(),
        noun_type: "DRGP".to_string(),
        data: AttrVal::IntegerType(100),
        old_data: Default::default(),
        user_name: "admin".to_string(),
        commit_comment: "Default session comment".to_string(),
    };
    let data = modify_data.convert_new_modify_data().unwrap();

    let mut file = fs::File::create("resource/sam7200_0001_test").unwrap();
    file.write_all(&data).unwrap();
}

#[test]
fn test_convert_new_modify_name_data() {
    let mut file = fs::File::open("resource/sam7200_0001").unwrap();
    let mut input = vec![];
    file.read_to_end(&mut input).unwrap();

    let modify_data = ModifyData {
        old_file: input,
        refno: RefU64::from_str("23584/5931").unwrap(),
        attr_type: "STWALL".to_string(),
        noun_type: "NAME".to_string(),
        data: AttrVal::StringType("/Test/WALL/Write".to_string()),
        old_data: AttrVal::StringType("/Test/WALL".to_string()),
        user_name: "admin".to_string(),
        commit_comment: "Default session comment".to_string(),
    };
    let data = modify_data.convert_new_modify_data().unwrap();

    let mut file = fs::File::create("resource/sam7200_0001_test").unwrap();
    file.write_all(&data).unwrap();
}
