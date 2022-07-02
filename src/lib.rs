#![feature(drain_filter)]

use std::any::TypeId;
use std::collections::BTreeSet;
use std::ops::Deref;
use lazy_static::lazy_static;
use dashmap::{DashMap, DashSet};
use aios_core::pdms_types::{AttrInfo, DbAttributeType};
use serde_json::from_str;
use aios_core::pdms_types::PdmsDatabaseInfo;
use aios_core::tool::db_tool::db1_dehash;
use dashmap::mapref::one::Ref;
use itertools::Itertools;
use nom::combinator::map;

pub mod tables;
pub mod database;
pub mod consts;
pub mod options;
pub mod helper;
pub mod query_sql;
pub mod sql;
pub mod api;
pub mod db_types;
pub mod data_interface;
pub mod cata;
pub mod ssc;

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