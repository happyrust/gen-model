use crate::cata::resolve::parse_to_u64;
use crate::data_to_file::modify::data_page::get_latest_data_page;
use crate::data_to_file::modify::modify::ModifyNewData;
use crate::data_to_file::modify::session_page::get_latest_session_page;
use crate::data_to_file::OldDataPage;
use aios_core::consts::EXPR_ATT_SET;
use aios_core::pdms_types::*;
use aios_core::tool::db_tool::db1_hash;
use aios_core::{AttrMap, AttrVal, PdmsDatabaseInfo};
use dashmap::DashMap;
use dashmap::DashSet;
use lazy_static::lazy_static;
use memchr::memmem::{find_iter, rfind_iter};
use nalgebra::inf;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::str::FromStr;

lazy_static! {
    /// attr_map 中不需要转为 bytes的属性
    pub static ref ATT_BYTES_SET: DashSet<NounHash> = {
        let mut set = DashSet::new();
        set.insert((db1_hash("OWNER")));
        set.insert((db1_hash("TYPE")));
        set
    };
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct DataPageIncrement {
    pub old_file: Vec<u8>,
    // 新增的节点所在的文件
    pub attr_type: String,
    // 新增的参考号的 attr_map
    pub attr: AttrMap,
    // 新增节点在当前层级中的位置
    pub order: usize,
    pub owner_refno: RefU64,
    pub owner_type: String,
    // pdms all_attr_info.json文件中的值
    pub info_map: PdmsDatabaseInfo,
}

impl DataPageIncrement {
    pub fn convert_new_increment_data_file(self) -> Option<Vec<u8>> {
        // 获得最新得参考号
        let refno = RefU64(parse_to_u64(&self.old_file[0x80C..0x814]));
        // 找到 owner 的数据页
        let owner_data_page =
            get_latest_data_page(&self.old_file, self.owner_refno, &self.owner_type);
        if owner_data_page.is_none() {
            return None;
        }
        let owner_data_page = owner_data_page.unwrap();
        // 修改 owner 新增变化的数据(主要是children)
        let data_page = change_owner_data(owner_data_page, self.order, refno);
        let owner_data_bytes = data_page.turn_self_into_vec();
        // 生成新节点的数据
        let new_node_bytes = convert_data_by_attr(self, refno);
        if new_node_bytes.is_none() {
            return None;
        }
        let new_node_bytes = new_node_bytes.unwrap();
        // 将两个bytes合并成 data_page
        let mut new_data_page = vec![0; 0x800];
        let data_types = [owner_data_bytes, new_node_bytes].concat();
        new_data_page.splice(0..data_types.len(), data_types);
        Some(new_data_page)
    }
}

/// 修改 owner data_page 中的数据
fn change_owner_data(mut owner_data: OldDataPage, order: usize, refno: RefU64) -> OldDataPage {
    // 根据 order 确定 在 owner 的 children 中排在第几个位置
    let children_len = (owner_data.children.len() - 20) / 8; // 20为长度和自身参考号 + 8个 0
    if order < children_len {
        // 在中间插入该参考号的数据
        let refno_position = order * 8 + 20;
        let before_refno_bytes = owner_data.children[..refno_position].to_vec();
        let refno_bytes = refno.0.to_be_bytes()[..8].to_vec();
        let after_bytes = owner_data.children[refno_position..].to_vec();
        owner_data.children = [before_refno_bytes, refno_bytes, after_bytes].concat();
    } else {
        // 在最后插入数据
        owner_data
            .children
            .append(&mut refno.0.to_be_bytes()[..8].to_vec());
    }
    owner_data.children.splice(
        2..4,
        ((owner_data.children.len() / 4) as u16).to_be_bytes()[..2].to_vec(),
    );
    owner_data
}

fn convert_data_by_attr(data_page: DataPageIncrement, new_refno: RefU64) -> Option<Vec<u8>> {
    // let mut implicit_head_data = vec![];
    // // 生成 参考号 + type + owner 的 bytes
    // implicit_head_data.append(&mut [new_refno.0.to_be_bytes()[..8].to_vec(),
    //     db1_hash(&data_page.attr_type).to_be_bytes()[..4].to_vec(), data_page.owner_refno.0.to_be_bytes()[..8].to_vec()].concat());
    // // 获取当前 data_page 的 page_num
    // let latest_session_page_num = parse_to_u32(&data_page.old_file[40..44]);
    // let latest_session_page = get_latest_session_page(&data_page.old_file, latest_session_page_num);
    // let latest_page_num = parse_to_u32(&latest_session_page[20..24]);
    // let current_data_page_num = latest_page_num + 1;
    // implicit_head_data.append(&mut current_data_page_num.to_be_bytes()[..4].to_vec());
    // // 未知含义的16个byte数据 暂时用 0 代替
    // implicit_head_data.append(&mut vec![0; 16]);
    // // 将显示属性和隐式属性分类 ， 新增的数据暂时不考虑他是否有子节点，默认为叶子节点
    // let new_data_page = OldDataPage::from_attr_map(new_refno, data_page.attr, data_page.info_map.noun_attr_info_map);
    // if new_data_page.is_none() { return None; }
    // let mut new_data_page = new_data_page.unwrap();
    // // 补全隐式属性开头的数据
    // new_data_page.implicit_data = [implicit_head_data, new_data_page.implicit_data].concat();
    // let len = new_data_page.implicit_data.len() / 4 + 1;
    // let head = (len as u32).to_be_bytes().to_vec();
    // new_data_page.implicit_data = [head, new_data_page.implicit_data].concat();
    // Some(new_data_page.turn_self_into_vec())
    None
}

/// 将显式属性转换为 bytes
pub fn convert_new_node_data_explicit(refno: RefU64, default_map: AttrMap, b_f64: bool) -> Vec<u8> {
    let mut r = vec![];
    // 开头的 参考号和 8 个 0
    r.push([refno.0.to_be_bytes()[..8].to_vec(), vec![0; 8]].concat());
    // 将显示属性的值转换为 bytes
    for (noun, val) in default_map.map {
        if ATT_BYTES_SET.contains(&noun) {
            continue;
        }
        let noun_hash_bytes = noun.to_be_bytes()[..4].to_vec();
        match val {
            AttrVal::IntegerType(v) => {
                r.push(ModifyNewData::convert_explicit_data_to_bytes(
                    noun_hash_bytes,
                    vec![0xC, 0, 0, 1],
                    None,
                    v.to_be_bytes()[..4].to_vec(),
                ));
            }
            AttrVal::WordType(v) => {
                if v != SmolStr::new("unset") {
                    let v = db1_hash(v.as_str()).to_be_bytes().to_vec();
                    r.push(ModifyNewData::convert_explicit_data_to_bytes(
                        noun_hash_bytes,
                        vec![0xC, 0, 0, 1],
                        None,
                        v,
                    ));
                }
            }
            AttrVal::StringType(v) => {
                if v != SmolStr::new("unset") {
                    let v = v.as_bytes();
                    let len = v.len() as f32;
                    let mut l = [
                        vec![0x3C, 0],
                        (((len / 4.0).ceil() + 1.0) as u16).to_be_bytes().to_vec(),
                    ]
                    .concat();
                    let len = (len as u32).to_be_bytes().to_vec();
                    r.push(ModifyNewData::convert_explicit_data_to_bytes(
                        noun_hash_bytes,
                        l,
                        Some(len),
                        v.to_vec(),
                    ));
                }
            }
            // bool 先不管
            // AttrVal::BoolType(v) => {
            //     if v {
            //         r.push(ModifyNewData::convert_explicit_data_to_bytes(noun_hash, vec![0x14, 0, 0, 1], None, vec![0, 0, 0, 1]));
            //     } else {
            //         r.push(ModifyNewData::convert_explicit_data_to_bytes(noun_hash, vec![0x14, 0, 0, 1], None, vec![0, 0, 0, 0]));
            //     }
            // }
            AttrVal::DoubleType(v) => {
                if let [a, b, c, d, e, f, g, h] = v.to_be_bytes() {
                    let value = vec![e, f, g, h, a, b, c, d];
                    r.push(ModifyNewData::convert_explicit_data_to_bytes(
                        noun_hash_bytes,
                        vec![8, 0, 0, 2],
                        None,
                        value,
                    ));
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
                r.push(ModifyNewData::convert_explicit_data_to_bytes(
                    noun_hash_bytes,
                    l,
                    Some(len),
                    value,
                ));
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
                r.push(ModifyNewData::convert_explicit_data_to_bytes(
                    noun_hash_bytes,
                    l,
                    Some(vec![0, 0, 0, 3]),
                    value,
                ));
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
                r.push(ModifyNewData::convert_explicit_data_to_bytes(
                    noun_hash_bytes,
                    l,
                    Some(len),
                    value,
                ));
            }
            _ => {}
        }
    }
    r.into_iter().flatten().collect()
}

/// 将隐式属性转换为 bytes
pub fn convert_new_node_data_implicit(
    attr_map: BTreeMap<u32, (NounHash, AttrVal)>,
    _b_f64: bool,
) -> Vec<u8> {
    let mut values = vec![];
    for (_, (noun, val)) in attr_map {
        if ATT_BYTES_SET.contains(&noun) {
            continue;
        }
        match &val {
            AttrVal::IntegerType(val) => {
                values.push(val.to_be_bytes()[..4].to_vec());
            }
            AttrVal::StringType(val) => {
                if !EXPR_ATT_SET.contains(&(noun as i32)) {
                    let v = val.as_str().as_bytes().to_vec();
                    let len = v.len() as f32;
                    let l = (((len / 4.0).ceil() + 1.0) as u16).to_be_bytes().to_vec();
                    let len = (len as u32).to_be_bytes().to_vec();
                    values.push([len, l, v].concat());
                } else {
                    values.push(vec![
                        0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    ]);
                }
            }
            AttrVal::DoubleType(v) => {
                if let [a, b, c, d, e, f, g, h] = v.to_be_bytes() {
                    let value = vec![e, f, g, h, a, b, c, d];
                    values.push(value)
                }
            }
            // bool 压缩的值先不管
            AttrVal::BoolType(v) => {
                // 这个noun是bool 但是默认值是 0C 不知道怎么搞的
                if noun == db1_hash("CLFL") {
                    values.push(vec![0, 0, 0, 0xC]);
                } else {
                    if *v {
                        values.push(vec![0, 0, 0, 1]);
                    } else {
                        values.push(vec![0, 0, 0, 0]);
                    }
                }
            }
            AttrVal::DoubleArrayType(value) => {
                let mut r = vec![];
                for v in value {
                    if let [a, b, c, d, e, f, g, h] = v.to_be_bytes() {
                        r.push(vec![e, f, g, h, a, b, c, d]);
                    }
                }
                let l = ((value.len() * 2 + 1) as u16).to_be_bytes()[..2].to_vec();
                let r = r.into_iter().flatten().collect::<Vec<u8>>();
                values.push([l, r].concat());
            }
            AttrVal::IntArrayType(value) => {
                let mut r = vec![];
                for v in value {
                    r.push(v.to_be_bytes().to_vec());
                }
                values.push((r.len() as u32).to_be_bytes().to_vec());
                let value = r.into_iter().flatten().collect::<Vec<u8>>();
                values.push(value);
            }
            AttrVal::Vec3Type(value) => {
                let mut r = vec![];
                r.push(vec![0, 0, 0, 3]);
                for v in value {
                    if let [a, b, c, d, e, f, g, h] = v.to_be_bytes() {
                        r.push(vec![e, f, g, h, a, b, c, d]);
                    }
                }
                let value = r.into_iter().flatten().collect();
                values.push(value);
            }
            // ElementType除了是refno，忘了还可能是什么情况，暂时只考虑了refno的情况
            AttrVal::ElementType(value) => {
                if let Ok(refno) = RefU64::from_str(&value.to_string()) {
                    values.push(refno.0.to_be_bytes().to_vec());
                } else {
                    values.push(vec![0; 8]);
                }
            }
            AttrVal::RefU64Type(value) => {
                values.push(value.0.to_be_bytes().to_vec());
            }

            _ => {}
        }
    }
    values.into_iter().flatten().collect()
}

/// 生成新增节点的参考号 + 版本号
fn convert_first_version_page_increment(
    input: &[u8],
    owner_refno: RefU64,
    refno: RefU64,
    version: u32,
) -> Option<Vec<u8>> {
    let version_start = &[
        0x0u8, 0x0, 0x0, 0x5, 0x0, 0xCC, 0x47, 0xDF, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x2, 0x0,
        0x0, 0x0, 0x2,
    ];
    let mut iter = rfind_iter(input, version_start);
    while let Some(pos) = iter.next() {
        let mut version_page = vec![0u8; 0x800];
        let mut version_data = input[pos..pos + 0x800].to_vec();
        let owner_refno = &owner_refno.0.to_be_bytes()[..];
        if let Some(_r_pos) = find_iter(&input[pos..pos + 0x800], owner_refno).next() {
            // 找到 page末尾数据为 0 的地方
            if let Some(zero_pos) = find_iter(&version_data, &vec![0, 0, 0, 0, 0, 0, 0, 0]).next() {
                let refno = refno.0.to_be_bytes()[..8].to_vec();
                let new_version = (version - 4).to_be_bytes()[..4].to_vec();
                let unknown_bytes = vec![0, 0x5, 0xA0, 0x1]; // 也是不知道是什么含义
                let new_data = [refno, new_version, unknown_bytes].concat();
                version_data.splice(zero_pos..zero_pos + 16, new_data);
                version_page.splice(0..0x800, version_data);
            }
            return Some(version_page);
        }
    }
    None
}
//
// #[test]
// fn test_convert_new_increment_data_file() {
//     let mut file = fs::File::open("resource/sam7200_0001").unwrap();
//     let mut input = vec![];
//     file.read_to_end(&mut input).unwrap();
//     let info = serde_json::from_str::<PdmsDatabaseInfo>(&include_str!("../../../all_attr_info.json")).unwrap();
//     dbg!(&info.noun_attr_info_map.len());
//     let mut attr = AttrMap::default();
//     attr.insert((db1_hash("TYPE")), AttrVal::StringType(SmolStr::new("ELBO")));
//
//     let increment_new_data = DataPageIncrement {
//         old_file: input,
//         attr_type: "ELBO".to_string(),
//         owner_refno: RefU64::from_str("23584/16355").unwrap(),
//         owner_type: "BRAN".to_string(),
//         attr,
//         info_map: info,
//         order: 0,
//     };
//
//     let data = increment_new_data.convert_new_increment_data_file().unwrap();
//     let mut file = fs::File::create("resource/sam7200_0001_increment_test").unwrap();
//     file.write_all(&data).unwrap();
//
//     // 期望的数据
//     let success_bytes = "00 00 00 07 00 00 00 56 00 00 5C 20 00 00 3F E3
// 00 0C 55 1C 00 00 5C 20 00 00 3F E2 00 00 12 17
// 00 0C 00 01 00 00 12 17 00 0A E0 01 20 06 40 04
// 00 00 00 00 00 00 00 0C 00 00 00 03 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 03 00 00 00 00 40 B3 88 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 03 00 00 00 00 3F F0 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 03
// 00 00 00 00 3F F0 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 40 54 00 00
// 00 00 00 00 40 54 00 00 00 0C 60 57 00 0C 60 57
// 00 00 00 00 00 00 00 00 00 00 00 00 C0 F8 6A 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 3B 58
// 00 03 80 1F 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 3B 58 00 03 80 1B
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 02 80 00 00 01 80 00 00 01 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 02 00 09
// 00 00 5C 20 00 00 3F E3 00 00 00 00 00 00 00 00
// 00 00 5C 20 00 00 3F EF 00 00 5C 20 00 00 3F E5
// 00 01 00 1E 00 00 5C 20 00 00 3F E3 00 00 00 00
// 00 00 00 00 00 0A AF CA 14 00 00 01 00 00 00 00
// 06 A0 26 04 0C 00 00 01 00 36 7E CC 05 57 EA 2C
// 40 00 00 02 00 00 00 00 00 00 00 00 10 71 D1 20
// 08 00 00 02 00 00 00 00 00 00 00 00 10 71 D1 2B
// 08 00 00 02 00 00 00 00 00 00 00 00 00 0D FD 22
// 14 00 00 01 00 00 00 00 00 CC 6B 3F 38 00 00 02
// 00 00 00 01 00 0C 55 1C 00 00 00 2B 00 00 5C 20
// 00 00 3F EF 00 0C A4 39 00 00 5C 20 00 00 3F E3
// 00 00 12 17 00 15 20 01 00 00 00 00 00 00 00 00
// 20 06 C0 00 00 00 00 03 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 03 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 01 00 00 00 02 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 40 56 80 00
// 00 00 00 00 00 00 00 00 00 00 00 02 4D 7C 94 10
// 80 00 00 01 00 01 00 20 00 00 5C 20 00 00 3F EF
// 00 00 00 00 00 00 00 00 00 0A AF CA 14 00 00 01
// 00 00 00 00 00 09 2E A7 0C 00 00 01 FF FF FF FF
// 00 0B C6 C0 14 00 00 01 00 00 00 01 06 A0 26 04
// 0C 00 00 01 00 36 7E CC 10 71 D1 20 08 00 00 02
// 00 00 00 00 00 00 00 00 10 71 D1 2B 08 00 00 02
// 00 00 00 00 00 00 00 00 00 0D FD 22 14 00 00 01
// 00 00 00 00 00 CC 6B 3F 38 00 00 02 00 00 00 01
// 00 0C A4 39 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
// 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 ";
//     let success_bytes = convert_str_to_bytes(success_bytes);
// }
