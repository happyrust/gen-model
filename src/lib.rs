#![feature(drain_filter)]

use std::any::TypeId;
use std::collections::BTreeSet;
use std::ops::Deref;
use lazy_static::lazy_static;
use dashmap::DashMap;
use aios_core::pdms_types::{AttrInfo, DbAttributeType};
use serde_json::from_str;
use aios_core::pdms_types::PdmsDatabaseInfo;
use aios_core::tool::db_tool::db1_dehash;
use dashmap::mapref::one::Ref;
use nom::combinator::map;

pub mod tables;
pub mod defs;
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

#[derive(Default, Debug, Clone)]
pub struct AttInfoMap{
    pub map: DashMap<i32, DashMap<i32, AttrInfo>>,
    pub type_att_names_map: DashMap<String, BTreeSet<String>>,
    pub att_name_type_map: DashMap<String, DbAttributeType>,
}

impl Deref for AttInfoMap {
    type Target = DashMap<i32, DashMap<i32, AttrInfo>>;

    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl AttInfoMap {
    #[inline]
    pub fn init_type_att_names_map(&mut self){
        for k in &self.map {
            let type_name = db1_dehash(*k.key() as u32);
            for v in k.value() {
                self.type_att_names_map.entry(type_name.clone())
                    .or_insert(BTreeSet::new()).insert(v.name.to_string());
                self.att_name_type_map.insert(v.name.to_string(), v.att_type);
            }
        }
    }

    #[inline]
    pub fn get_names_map(&self) -> &DashMap<String, BTreeSet<String>> {
        &self.type_att_names_map
    }

    #[inline]
    pub fn get_names_of_type(&self, type_name: &str) -> Option<Ref<String, BTreeSet<String>>> {
        self.type_att_names_map.get(type_name)
    }

    #[inline]
    pub fn exist_att_by_name(&self, type_name: &str, att_name: &str) -> bool {
        self.type_att_names_map.get(type_name).map(|x| x.contains(att_name)).unwrap_or(false)
    }

    /// 至少有一个 name 存在
    #[inline]
    pub fn exist_least_one_att_by_names(&self, type_name: &str, att_names: &Vec<&str>) -> bool {
        self.type_att_names_map.get(type_name).map(|x|
            att_names.iter().any(|v| x.value().contains(*v))).unwrap_or(false)
    }

    #[inline]
    pub fn get_val_type_of_att(&self, att_name: &str) -> Option<Ref<String, DbAttributeType>> {
        self.att_name_type_map.get(att_name)
    }
}

lazy_static! {
    static ref ATTR_INFO_MAP: AttInfoMap = {
        let db_info: PdmsDatabaseInfo = serde_json::from_str(include_str!("../all_attr_info.json")).unwrap();
        //调用方法
        let mut att_info_map = AttInfoMap{
            map: db_info.noun_attr_info_map,
            type_att_names_map: Default::default(),
            att_name_type_map: Default::default()
        };
        att_info_map.init_type_att_names_map();
        att_info_map
    };
}

pub const BATCH_CHUNKS_CNT: usize = 50;