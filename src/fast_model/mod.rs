pub mod gen_model;
// pub mod gen_model_refactored;
// pub mod gen_model_impl;

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

pub mod aabb_tree;

pub mod incremental;

pub mod aabb_cache;
pub mod session;

use aios_core::RefU64;
use dashmap::{DashMap, DashSet};
use once_cell::sync::Lazy;
use parry3d::bounding_volume::Aabb;
pub use gen_model::*;
// pub use gen_model_refactored::DbModelInstRefnos;
pub use occ_generate::*;
pub use query::*;
pub use resolve::*;

pub const SEND_INST_SIZE: usize = 500;
pub static EXIST_MESH_GEO_HASHES: Lazy<DashMap<String, Aabb>> = Lazy::new(DashMap::new);
