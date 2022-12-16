#![feature(drain_filter)]
#![feature(let_chains)]
#![feature(default_free_fn)]

use std::any::TypeId;
use std::collections::BTreeSet;
use std::ops::Deref;
use std::time::Instant;
use lazy_static::lazy_static;
use dashmap::{DashMap, DashSet};
use aios_core::pdms_types::{AttrInfo, DbAttributeType, RefU64};
use serde_json::from_str;
use aios_core::pdms_types::PdmsDatabaseInfo;
use aios_core::tool::db_tool::db1_dehash;
use dashmap::mapref::one::Ref;
use itertools::Itertools;
use nom::combinator::map;
use aios_core::pdms_data::AttInfoMap;
use anyhow::anyhow;
use sled::IVec;

pub mod tables;
pub mod database;
pub mod consts;
pub mod options;
pub mod helper;
pub mod api;
pub mod aql_api;
pub mod data_interface;
pub mod cata;
pub mod ssc;
pub mod defines;
pub mod graph_db;
pub mod plot_data;
pub mod data_to_file;
pub mod admin;
pub mod mdb;
pub mod data_to_excel;


#[macro_use]
extern crate derive_more;

#[macro_use]
extern crate nom;

lazy_static! {
    static ref ATTR_INFO_MAP: AttInfoMap = {
        let db_info: PdmsDatabaseInfo = serde_json::from_str(include_str!("../all_attr_info.json")).unwrap();
        //调用方法
        let mut att_info_map = AttInfoMap{
            map: db_info.noun_attr_info_map,
            type_att_names_map: Default::default(),
            type_explicit_att_names_map: Default::default(),
            att_name_type_map: Default::default(),
            has_cat_ref_types_set: Default::default(),
        };
        att_info_map.init_type_att_names_map();
        att_info_map
    };
}

pub const BATCH_CHUNKS_CNT: usize = 50;


//
// ///将当前的缓存数据，保存到sled，方便下次使用
// pub fn save_to_cache_sled_db() -> anyhow::Result<()> {
//     let mut time = Instant::now();
//     let db = sled::open(CACHE_SLED_NAME)?;
//     let attrmap_tree: sled::Tree = db.open_tree(b"ATTR_MAP_CACHE")?;
//     let mut batch = sled::Batch::default();
//     for k in PDMS_ATT_MAP_CACHE.iter() {
//         batch.insert(k.key(), k.value());
//     }
//     attrmap_tree.apply_batch(batch).map_err(|e| anyhow!(e.to_string()))?;
//
//     let attrmap_tree: sled::Tree = db.open_tree(b"IMPLICIT_ATTR_MAP_CACHE")?;
//     let mut batch = sled::Batch::default();
//     for k in PDMS_IMPLICIT_ATT_MAP_CACHE.iter() {
//         batch.insert(k.key(), k.value());
//     }
//     attrmap_tree.apply_batch(batch).map_err(|e| anyhow!(e.to_string()))?;
//
//     let refno_basic_tree: sled::Tree = db.open_tree(b"REFNO_BASIC_CACHE")?;
//     let mut batch = sled::Batch::default();
//     for k in CACHED_REFNO_BASIC_MAP.iter() {
//         batch.insert(k.key(), k.value());
//     }
//     refno_basic_tree.apply_batch(batch).map_err(|e| anyhow!(e.to_string()))?;
//
//     println!("缓存到db文件花费：{} ms", time.elapsed().as_millis());
//     Ok(())
// }
//
// ///从sled加载缓存数据
// pub fn load_from_cache_sled_db() -> anyhow::Result<()> {
//     let mut time = Instant::now();
//     let db = sled::open(CACHE_SLED_NAME)?;
//     let attrmap_tree: sled::Tree = db.open_tree(b"ATTR_MAP_CACHE")?;
//
//     for k in attrmap_tree.iter() {
//         if let Ok((key, value)) = k {
//             PDMS_ATT_MAP_CACHE.insert(key.into(), value.into());
//         }
//     }
//
//     let attrmap_tree: sled::Tree = db.open_tree(b"IMPLICIT_ATTR_MAP_CACHE")?;
//
//     for k in attrmap_tree.iter() {
//         if let Ok((key, value)) = k {
//             PDMS_IMPLICIT_ATT_MAP_CACHE.insert(key.into(), value.into());
//         }
//     }
//
//     let refno_basic_tree: sled::Tree = db.open_tree(b"REFNO_BASIC_CACHE")?;
//
//     for k in refno_basic_tree.iter() {
//         if let Ok((key, value)) = k {
//             CACHED_REFNO_BASIC_MAP.insert(key.into(), value.into());
//         }
//     }
//
//     println!("加载缓存db文件花费：{} ms", time.elapsed().as_millis());
//     Ok(())
//     // let mut batch = sled::Batch::default();
//     // for k in PDMS_ATT_MAP_CACHE.iter() {
//     //     batch.insert(&k.key().0.to_be_bytes(), bincode::serialize(k.value()).unwrap());
//     // }
//     // attrmap_tree.apply_batch(batch).map_err(|e| e.into())
// }