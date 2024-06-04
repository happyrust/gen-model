#![feature(let_chains)]
#![feature(async_closure)]
#![feature(exact_size_is_empty)]
#![feature(slice_take)]
#![feature(const_async_blocks)]
#![feature(type_alias_impl_trait)]
// 暂时屏蔽warnings
#![allow(warnings)]

#![recursion_limit = "256"]

use std::any::TypeId;
use std::collections::BTreeSet;
use std::ops::Deref;
use std::time::Instant;
use lazy_static::lazy_static;
use dashmap::{DashMap, DashSet};
use aios_core::pdms_types::*;
use serde_json::from_str;
use aios_core::tool::db_tool::db1_dehash;
use dashmap::mapref::one::Ref;
use itertools::Itertools;
use nom::combinator::map;
use aios_core::pdms_data::AttInfoMap;
use anyhow::anyhow;
use sled::IVec;

pub mod api;
pub mod tables;
pub mod consts;
pub mod data_interface;
pub mod cata;
// pub mod ssc;
pub mod defines;


pub mod graph_db;

#[cfg(feature = "gen_model")]
pub mod fast_model;

pub mod versioned_db;

pub mod mqtt_service;

#[macro_use]
extern crate derive_more;

#[macro_use]
extern crate nom;

#[macro_use]
extern crate strum_macros;
