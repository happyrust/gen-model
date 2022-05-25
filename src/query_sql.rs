use std::collections::BTreeMap;
use std::env;
use aios_core::pdms_types::{AiosStr, AttrMap, EleNode, RefU64};
use crate::db_types::EleNodeTIDB;
use smol_str::SmolStr;
use sqlx::{MySql, Pool, Row};
use crate::api::{element};
use crate::api::element::query_refno_type;
use crate::consts::PDMS_INFO_DB;
use crate::data_interface::tidb_manager::AiosDBManager;


