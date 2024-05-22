pub mod gen_model;

pub mod cata_model;

pub mod prim_model;

pub mod loop_model;

pub mod shared;

pub mod occ_generate;

pub mod manifold_bool;

pub mod room_model;

pub mod cal_model;

pub mod pdms_inst;

pub mod resolve;

pub mod query;

pub mod utils;

use aios_core::RefU64;
use dashmap::{DashMap, DashSet};
use once_cell::sync::Lazy;
pub use gen_model::*;
pub use occ_generate::*;
pub use query::*;
pub use resolve::*;


pub static EXIST_MESH_GEO_HASHES: Lazy<DashSet<String>> = Lazy::new(DashSet::new);