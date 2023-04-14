#![feature(drain_filter)]
#![feature(let_chains)]
#![feature(default_free_fn)]
#![feature(async_closure)]

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
pub mod pcf;
pub mod rvm;
pub mod metadata;
pub mod data_center_api;
pub mod spatial_tree;
pub mod negative;
pub mod ansys;

pub mod test;


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
pub const AQL_PDMS_ELES_COLLECTION: &'static str = "pdms_eles";