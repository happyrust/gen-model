#![feature(drain_filter)]
#![feature(let_chains)]
#![feature(default_free_fn)]
#![feature(async_closure)]
#![feature(exact_size_is_empty)]
// 暂时屏蔽warnings
#![allow(warnings)]

#![recursion_limit = "256"]

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
pub mod other_plat;

pub mod viewer;

#[macro_use]
extern crate derive_more;

#[macro_use]
extern crate nom;


pub const BATCH_CHUNKS_CNT: usize = 50;
pub const AQL_PDMS_ELES_COLLECTION: &'static str = "pdms_eles";
pub const AQL_PDMS_INST_COLLECTION: &'static str = "pdms_instances";
