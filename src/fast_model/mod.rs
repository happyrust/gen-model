pub mod gen_model;

pub mod concurrency;

pub mod cata_model;

pub mod prim_model;

pub mod loop_model;

pub mod shared;

pub mod occ_generate;

pub mod libgm_discretise;

pub mod mesh_primitives;

pub mod sweep_mesh;

#[cfg(test)]
pub mod mesh_assert;

pub mod manifold_bool;

#[cfg(feature = "manifold")]
pub mod manifold_csg;

#[cfg(feature = "manifold")]
pub mod manifold_tessellate;

pub mod room_fixture;

pub mod room_live_issue7;

pub mod room_predicate;

pub mod room_model;

pub mod cal_model;

pub mod pdms_inst;

pub(crate) mod shape_save;

pub mod resolve;

pub mod query;

pub mod utils;

pub mod aabb_tree;

pub mod spatial_state;

pub mod coverage_audit;

use aios_core::RefU64;
use dashmap::{DashMap, DashSet};
pub use gen_model::*;
pub use occ_generate::*;
use once_cell::sync::Lazy;
use parry3d::bounding_volume::Aabb;
pub use query::*;
pub use resolve::*;

pub const SEND_INST_SIZE: usize = 500;
pub static EXIST_MESH_GEO_HASHES: Lazy<DashMap<String, Aabb>> = Lazy::new(DashMap::new);
