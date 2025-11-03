use crate::api::attr::query_implicit_attr;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use aios_core::consts::EXPR_ATT_SET;
use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use aios_core::tool::db_tool::db1_hash;
use aios_core::{AttrMap, AttrVal};
use futures::future::ok;
use serde::{Deserialize, Serialize};
use std::env;
use std::io::Write;
use std::str::FromStr;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAttrMap {
    pub implicit_map: Vec<u8>,
    pub children: Vec<u8>,
    pub explicit_map: Vec<u8>,
}

/// 生成隐式属性
// 调用该方法之前就需要将显示和隐式分开
pub fn gen_implicit_attr_data(attr: AttrMap) -> Vec<u8> {
    let mut values = vec![];
    // 生成属性值
    for (hash, val) in attr.map {
        match &val {
            AttrVal::IntegerType(val) => {
                values.push(val.to_be_bytes()[..4].to_vec());
            }
            AttrVal::StringType(val) => {
                if !EXPR_ATT_SET.contains(&(hash as i32)) {
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
            // bool 先不管
            AttrVal::BoolType(v) => {}
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

#[tokio::test]
async fn test_gen_implicit_attr_data() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()
        .unwrap();
    let db_option: DbOption = s.try_deserialize().unwrap();
    let aios_mgr = AiosDBManager::init(&db_option).await?;
    let refno = RefU64::from_str("23584/5502").unwrap();
    if let Some(refno_basic) = aios_mgr.get_refno_basic(refno) {
        let implicit_map = query_implicit_attr(refno, refno_basic.value(), &pool, None).await?;
        let mut data = gen_implicit_attr_data(implicit_map);
        let mut file = std::fs::File::create("test_implicit.bin")?;
        file.write_all(&mut data)?;
    }
    Ok(())
}

/// 生成children
pub fn gen_children_data(refno: RefU64, children: Option<Vec<RefU64>>) -> Vec<u8> {
    let mut result = vec![];
    if let Some(children) = children {
        // 生成 0 2 加 长度
        let mut len = ((children.len() + 8) as u16).to_be_bytes()[..2].to_vec();
        let mut children_start_sign = vec![0u8, 2];
        children_start_sign.append(&mut len);
        result.append(&mut children_start_sign);
        // 创建自己refno + 8个 byte 0
        result.append(&mut refno.to_be_bytes().to_vec());
        result.append(&mut vec![0, 0, 0, 0, 0, 0, 0, 0]);
        // 创建children
        for child in children {
            result.append(&mut child.to_be_bytes().to_vec());
        }
    }
    result
}
