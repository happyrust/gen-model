use lazy_static::lazy_static;
use dashmap::DashMap;
use aios_core::pdms_types::AttrInfo;
use serde_json::from_str;
use aios_core::pdms_types::PdmsDatabaseInfo;

pub mod tables;
pub mod defs;
pub mod database;
pub mod consts;
pub mod insert_sql;
pub mod options;
pub mod helper;
pub mod query_sql;
pub mod sql;
pub mod api;
pub mod db_types;

lazy_static! {
    static ref REFNO_INFO_MAP: DashMap<i32, DashMap<i32, AttrInfo>> = {
        let db_info :PdmsDatabaseInfo = serde_json::from_slice(include_bytes!("../all_attr_info.json")).unwrap();
        db_info.noun_attr_info_map
    };
}

pub const BATCH_CHUNKS_CNT: usize = 50;