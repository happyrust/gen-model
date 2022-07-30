use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use aios_core::helper::{parse_to_u16, parse_to_u32};
use aios_core::pdms_types::{AttrVal, RefI32Tuple, RefU64};
use aios_core::tool::db_tool::db1_hash;
use dashmap::DashMap;
use memchr::memmem::{find_iter, rfind_iter};
use smol_str::SmolStr;
use crate::data_to_file::DataPage;
use crate::data_to_file::modify::{find_data_in_origin_file, get_origin_children_and_explicit_data, ModifyNewData};
use crate::EXPR_ATT_SET;
use crate::test_cases::convert_str_to_bytes;

pub struct IncrementDataPage {
    pub father_data: DataPage,
    pub child_data: DataPage,
}

impl IncrementDataPage {
    pub fn convert_new_data_page(self) -> Vec<u8> {
        let mut result = vec![0; 0x800];
        let f = self.father_data.turn_self_into_vec();
        let f_len = f.len();
        let s = self.child_data.turn_self_into_vec();
        let s_len = s.len();

        result.splice(..f_len, f);
        result.splice(f_len..s_len, s);
        result
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IncrementNewData {
    pub refno: RefU64,
    pub attr_type: String,
    pub owner_refno: RefU64,
    pub owner_type: String,
}

#[tokio::test]
async fn convert_new_node_data_test() -> anyhow::Result<()> {
    let data = IncrementNewData {
        refno: Refi32Tuple((23984, 1068)).into(),
        attr_type: "SCYL".to_string(),
        owner_refno: Refi32Tuple((23984, 1041)).into(),
        owner_type: "GMSE".to_string(),
    };
    let mut file = fs::File::open(r"E:\AVEVA\Plant\PDMS12.0.SP4\project\Sample\sam000\sam7600_0001").unwrap();
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok();
    let mut interface = PdmsInterface::new("mongodb://localhost:27017");
    if let Some(owner) = get_owner_data(&buf, data.clone()) {
        if let Some(mut version) = interface.get_file_version_with_refno(data.owner_refno.clone()).await? {
            version +=5;
            if let Some(r) = convert_first_version_page_increment(&buf,data.owner_refno,data.refno,version){
                fs::write("increment_version_page", r);
            }
            // let child_data = convert_new_node_data(data, version).await?;
            // let owner = owner.turn_self_into_vec();
            // let mut r = [owner, child_data].concat();
            // fs::write("increment", r);
        }
    }
    Ok(())
}

/// 找到owner的原数据并新增一个节点
fn get_owner_data(input: &[u8], node: IncrementNewData) -> Option<DataPage> {
    let node_bytes = get_refno_types_bytes(node.owner_refno, node.owner_type);
    if let Some(origin_data) = find_data_in_origin_file(&node_bytes, &input) {
        let new_owner = change_owner_children_data(origin_data.children, node.refno);
        return Some(DataPage {
            implicit_data: origin_data.implicit_data,
            children: new_owner,
            explicit_data: origin_data.explicit_data,
        });
    }
    None
}

/// 生成新增节点的数据
async fn convert_new_node_data(node: IncrementNewData, version: u32) -> MResult<Vec<u8>> {
    let node_bytes = get_refno_types_bytes(node.refno.clone(), node.attr_type);
    let owner_byte = node.owner_refno.0.to_be_bytes()[..8].to_vec(); // owner 不需要type
    let version = version.to_be_bytes()[..4].to_vec();
    let unknown_byte = vec![0, 9, 0x20, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0x20, 0, 0xC0, 0]; // 版本号之后 隐式属性之前有16个byte未知含义的数据

    let mut interface = PdmsInterface::new("mongodb://localhost:27017");
    let default_val_map = interface.get_type_default_value(SmolStr::new("SCYL")).await?.unwrap();
    let (explicit, implicit) = sort_default_value(default_val_map);

    let implicit_data = convert_new_node_default_data_implicit(implicit, true);
    let implicit_len = ((implicit_data.len() as u32) / 4 + 0xB).to_be_bytes().to_vec();
    let explicit_value = convert_new_node_default_data_explicit(explicit, true);
    let explicit_data = get_explicit_data(explicit_value, node.refno);
    Ok([implicit_len, node_bytes, owner_byte, version, unknown_byte, implicit_data, explicit_data].concat())
}

fn get_refno_types_bytes(refno: RefU64, att_type: String) -> Vec<u8> {
    let r = refno.0.to_be_bytes()[..8].to_vec();
    let a = db1_hash(att_type.as_str()).to_be_bytes()[..4].to_vec();
    [r, a].concat()
}

fn get_explicit_data(input: Vec<u8>, refno: RefU64) -> Vec<u8> {
    let mut r = refno.0.to_be_bytes()[..8].to_vec();
    r.append(&mut vec![0, 0, 0, 0, 0, 0, 0, 0]);
    let len = (input.len() as u16).to_be_bytes().to_vec();
    [vec![0, 1], len, r, input].concat()
}

#[test]
fn change_owner_data_test() {
    let input = "
00 02 00 1D 00 00 5D B0 00 00 04 11 00 00 00 00
00 00 00 00 00 00 5D B0 00 00 04 2C 00 00 5D B0 00 00 04 29
00 00 5D B0 00 00 04 23 00 00 5D B0 00 00 04 21
00 00 5D B0 00 00 04 20 00 00 5D B0 00 00 04 1F
00 00 5D B0 00 00 04 1C 00 00 5D B0 00 00 04 1A
00 00 5D B0 00 00 04 17 00 00 5D B0 00 00 04 16
00 00 5D B0 00 00 04 13 00 00 5D B0 00 00 04 12";
    let input = convert_str_to_bytes(input);
    let owner = change_owner_children_data(input, RefI32Tuple((23984, 1068)).into());
    println!("owner={:#4X?}", owner);
}

fn change_owner_children_data(mut input: Vec<u8>, refno: RefU64) -> Vec<u8> {
    let old_len = parse_to_u16(&input[2..4]);
    let new_len = (old_len + 2).to_be_bytes().to_vec();
    input.splice(2..4, new_len);
    let refno_byte = refno.0.to_be_bytes()[..8].to_vec();
    [input, refno_byte].concat()
}

#[tokio::test]
async fn convert_new_node_default_data_test() {
    let pdms_database_info = read_attr_info_config("all_attr_info.bin");
    let mut interface = PdmsInterface::new("mongodb://localhost:27017");
    let default_val_map = interface.get_type_default_value(SmolStr::new("SCYL")).await.unwrap().unwrap();
    let (explicit, implicit) = sort_default_value(default_val_map);
    println!("explicit={:?}", implicit);
    let r = convert_new_node_default_data_explicit(explicit, true);
    println!("r={:#4X?}", r);
    let r = convert_new_node_default_data_implicit(implicit, true);
    println!("r={:#4X?}", r);
}

fn convert_new_node_default_data_explicit(default_map: DashMap<SmolStr, AttrVal>, b_f64: bool) -> Vec<u8> {
    let mut r = vec![];
    for (noun, val) in default_map {
        let noun_hash = db1_hash(noun.as_str()).to_be_bytes()[..4].to_vec();
        match val {
            AttrVal::IntegerType(v) => {
                r.push(ModifyNewData::convert_explicit_data_to_bytes(noun_hash, vec![0xC, 0, 0, 1], None, v.to_be_bytes()[..4].to_vec()));
            }
            AttrVal::WordType(v) => {
                if v != SmolStr::new("unset") {
                    let v = db1_hash(v.as_str()).to_be_bytes().to_vec();
                    r.push(ModifyNewData::convert_explicit_data_to_bytes(noun_hash, vec![0xC, 0, 0, 1], None, v));
                }
            }
            AttrVal::StringType(v) => {
                if v != SmolStr::new("unset") {
                    let v = v.as_bytes();
                    let len = v.len() as f32;
                    let mut l = [vec![0x3C, 0], (((len / 4.0).ceil() + 1.0) as u16).to_be_bytes().to_vec()].concat();
                    let len = (len as u32).to_be_bytes().to_vec();
                    r.push(ModifyNewData::convert_explicit_data_to_bytes(noun_hash, l, Some(len), v.to_vec()));
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
                    r.push(ModifyNewData::convert_explicit_data_to_bytes(noun_hash, vec![8, 0, 0, 2], None, value));
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
                r.push(ModifyNewData::convert_explicit_data_to_bytes(noun_hash, l, Some(len), value));
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
                r.push(ModifyNewData::convert_explicit_data_to_bytes(noun_hash, l, Some(vec![0, 0, 0, 3]), value));
            }
            AttrVal::IntArrayType(values) => {
                let mut value = vec![];
                for v in values {
                    value.push(v.to_be_bytes().to_vec());
                }
                let len = (value.len() as u32).to_be_bytes().to_vec();
                let l = [vec![0x0, 0], ((value.len() + 1) as u16).to_be_bytes()[..2].to_vec()].concat(); // 还没找到pdms文件中的IntArrayType数据
                let value = value.into_iter().flatten().collect::<Vec<u8>>();
                r.push(ModifyNewData::convert_explicit_data_to_bytes(noun_hash, l, Some(len), value));
            }
            _ => {}
        }
    }
    r.into_iter().flatten().collect()
}

/// 生成新增节点的默认隐式属性属性值
fn convert_new_node_default_data_implicit(default_map: BTreeMap<u32, (SmolStr, AttrVal)>, _b_f64: bool) -> Vec<u8> {
    let mut values = vec![];
    for (_, (noun, val)) in default_map {
        match &val {
            AttrVal::IntegerType(val) => {
                values.push(val.to_be_bytes()[..4].to_vec());
            }
            AttrVal::StringType(val) => {
                if !EXPR_ATT_SET.contains(&(db1_hash(noun.as_str()) as i32)) {
                    let v = val.as_str().as_bytes().to_vec();
                    let len = v.len() as f32;
                    let l = (((len / 4.0).ceil() + 1.0) as u16).to_be_bytes().to_vec();
                    let len = (len as u32).to_be_bytes().to_vec();
                    values.push([len, l, v].concat());
                } else {
                    values.push(vec![0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
                }
            }
            AttrVal::DoubleType(v) => {
                if let [a, b, c, d, e, f, g, h] = v.to_be_bytes() {
                    let value = vec![e, f, g, h, a, b, c, d];
                    values.push(value)
                }
            }
            // bool 先不管
            AttrVal::BoolType(v) => {
                // 这个noun是bool 但是默认值是 0C 不知道怎么搞的
                if noun == SmolStr::new("CLFL") {
                    values.push(vec![0, 0, 0, 0xC]);
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
                let mut l = vec![];
                for v in value {
                    if let [a, b, c, d, e, f, g, h] = v.to_be_bytes() {
                        r.push(vec![e, f, g, h, a, b, c, d]);
                    }
                }
                l = ((r.len() * 2 + 1) as u16).to_be_bytes()[..2].to_vec();
                l = [vec![0x18, 0], l].concat();
                let value = r.into_iter().flatten().collect();
                values.push(value);
            }
            _ => {}
        }
    }
    values.into_iter().flatten().collect()
}

/// 根据pos获取到节点的隐式属性、children、显示属性
pub fn get_data_page_with_pos(input: &[u8], pos: usize) -> DataPage {
    let mut implicit_pos = pos - 4 + (parse_to_u32(&input[pos - 4..pos]) as usize) * 4;
    let implicit_data = input[pos - 4..implicit_pos].to_vec();
    let (children_data, explicit_data) = get_origin_children_and_explicit_data(input, implicit_pos);
    DataPage {
        implicit_data,
        children: children_data,
        explicit_data,
    }
}

/// 生成 refno + type 在 pdms中的数据
pub fn convert_refno_type_hash(refno: RefU64, attr_type: String) -> Vec<u8> {
    let mut refno = refno.0.to_be_bytes().to_vec();
    let attr_type = db1_hash(attr_type.as_str()).to_be_bytes().to_vec();
    [refno, attr_type].concat()
}

/// 返回排序后的显示属性和隐式属性的默认值
fn sort_default_value(map: DashMap<SmolStr, DefaultValue>) -> (DashMap<SmolStr, AttrVal>, BTreeMap<u32, (SmolStr, AttrVal)>) {
    let mut explicit_map = DashMap::new(); // 显示属性只需要存 noun和默认值 不需要管顺序
    let mut implicit_map = BTreeMap::new(); // 隐式属性存offset和 默认值，noun只是方便调试
    for (key, val) in map {
        // bool 先不管
        if val.offset == 0 {
            explicit_map.insert(key, val.value);
        } else if val.offset < 0xFFF {
            implicit_map.insert(val.offset, (key, val.value));
        }
    }
    (explicit_map, implicit_map)
}

/// 生成新增节点的参考号 + 版本号
fn convert_first_version_page_increment(input: &[u8], owner_refno: RefU64,refno:RefU64, version: u32) -> Option<Vec<u8>>{
    let version_start = &[0x0u8, 0x0, 0x0, 0x5, 0x0, 0xCC, 0x47, 0xDF, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x2, 0x0, 0x0, 0x0, 0x2];
    let mut iter = rfind_iter(input, version_start);
    while let Some(pos) = iter.next() {
        let mut version_page = vec![0u8; 0x800];
        let mut version_data = input[pos..pos + 0x800].to_vec();
        let owner_refno = &owner_refno.0.to_be_bytes()[..];
        if let Some(_r_pos) = find_iter(&input[pos..pos + 0x800], owner_refno).next() {
            // 找到 page末尾数据为 0 的地方
            if let Some(zero_pos) = find_iter(&version_data,&vec![0,0,0,0,0,0,0,0]).next() {
                let refno = refno.0.to_be_bytes()[..8].to_vec();
                let new_version = (version - 4).to_be_bytes()[..4].to_vec();
                let unknown_bytes = vec![0 ,0x5 ,0xA0 ,0x1]; // 也是不知道是什么含义
                let new_data = [refno,new_version,unknown_bytes].concat();
                version_data.splice(zero_pos..zero_pos + 16,new_data);
                version_page.splice(0..0x800, version_data);
            }
            return Some(version_page);
        }
    }
    None
}