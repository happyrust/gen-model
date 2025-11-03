use std::collections::{BTreeSet, HashMap, HashSet};
use std::env;
use std::sync::Arc;
use aios_core::{AttrVal, RefU64Vec};
use aios_core::db_number::DbNumMgr;
use aios_core::helper::table::qualified_table_name;
use aios_core::pdms_types::*;
use anyhow::anyhow;

use crate::consts::*;
use dashmap::DashMap;
use sqlx::{Error, MySql, Pool, Row};
use crate::api::attr::query_explicit_attr;
use crate::api::element::query_types_refnos;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::arangodb::ArDatabase;


const WDJZ: i32 = 642952044;

///更新获得ref0->projects 缓存
pub async fn get_ref0_projects(pool: &Pool<MySql>) -> anyhow::Result<DashMap<u32, Vec<String>>> {
    let mut map = DashMap::new();
    let sql = format!("SELECT REF0, PROJECT FROM {PDMS_REFNO_INFOS_TABLE}");
    // dbg!(&sql);
    let results = sqlx::query(&sql).fetch_all(pool).await;
    match results {
        Ok(vals) => {
            for val in vals {
                let ref0 = val.get::<i32, _>("REF0") as u32;
                let project_str = val.get::<String, _>("PROJECT");
                map.entry(ref0).or_insert(Vec::new()).push(project_str);
            }
        }
        Err(e) => {
            dbg!(e);
            dbg!(sql);
        }
    }
    Ok(map)
}

// pub async fn sync_local_refno_basic_map(att_db: sled::Tree) -> anyhow::Result<bool> {
//
//
//
//     let sql = format!("SELECT ID, OWNER, TYPE  FROM {PDMS_ELEMENTS_TABLE}");
//     let results = sqlx::query(&sql).fetch_all(pool).await;
//     match results {
//         Ok(vals) => {
//             for val in vals {
//                 let refno = (val.get::<i64, _>("ID") as u64).into();
//                 let owner = (val.get::<i64, _>("OWNER") as u64).into();
//                 let type_name = val.get::<String, _>("TYPE");
//                 let table = qualified_table_name(type_name.as_str());
//                 if CACHED_REFNO_BASIC_MAP.get(&refno).is_none() {
//                     let _ = CACHED_REFNO_BASIC_MAP.insert(refno, &CachedRefBasic {
//                         owner,
//                         table,
//                     });
//                 }
//             }
//         }
//         Err(e) => {
//             dbg!(&e);
//             dbg!(sql);
//             return Err(anyhow::anyhow!(e.to_string()));
//         }
//     }
//     Ok(true)
// }

// 已废弃: cache 模块已移除
// /// 获取生成refno到RefBasic的映射, todo 存储有点慢，需要批量存储
// pub async fn sync_refno_basic_map(pool: &Pool<MySql>) -> anyhow::Result<bool> {
//     let sql = format!("SELECT ID, OWNER, TYPE  FROM {PDMS_ELEMENTS_TABLE}");
//     let results = sqlx::query(&sql).fetch_all(pool).await;
//     match results {
//         Ok(vals) => {
//             for val in vals {
//                 let refno = (val.get::<i64, _>("ID") as u64).into();
//                 let owner = (val.get::<i64, _>("OWNER") as u64).into();
//                 let type_name = val.get::<String, _>("TYPE");
//                 let table = qualified_table_name(type_name.as_str());
//                 if CACHED_REFNO_BASIC_MAP.get(&refno).is_none() {
//                     let _ = CACHED_REFNO_BASIC_MAP.insert(refno, &CachedRefBasic {
//                         owner,
//                         table,
//                     });
//                 }
//             }
//         }
//         Err(e) => {
//             dbg!(&e);
//             dbg!(sql);
//             return Err(anyhow::anyhow!(e.to_string()));
//         }
//     }
//     Ok(true)
// }


/// 通过uda，获取设备的底标高
pub async fn query_refno_height_position(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<String> {
    let explicit_attr = query_explicit_attr(refno, pool).await?;
    let position = explicit_attr.get(&(WDJZ as u32));
    if let Some(AttrVal::StringType(position)) = position {
        let position = position.replace("mm","").trim().to_string();
        Ok(position.to_string())
    } else {
        Ok("0.0".to_string())
    }
}

fn gen_query_refnos_implicit_string_attr(table_name: &str, value: Vec<&str>, refnos: RefU64Vec) -> String {
    let mut filed = String::from("ID ,".to_string());
    for v in value {
        filed.push_str(&format!("{} ,", v))
    }
    filed.remove(filed.len() - 1);

    let mut refno_strs = String::new();
    for refno in refnos {
        refno_strs.push_str(&format!("{} ,", refno.0));
    }
    refno_strs.remove(refno_strs.len() - 1);

    let mut sql = String::new();
    sql.push_str(&format!("SELECT {} FROM {table_name} WHERE ID IN ( {} )", filed, refno_strs));
    sql
}