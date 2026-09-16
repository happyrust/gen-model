pub mod e3d_mesh_store;
pub mod e3d_model_service;
#[cfg(feature = "legacy_model")]
pub(crate) mod gen_model;
pub mod historical_model;
pub(crate) mod model_artifact;
pub mod model_db_adapter;
pub mod model_memory_store;
pub mod model_record_reader;
pub(crate) mod model_write_behind;
/// 初始化直写的模型侧渲染（ADR-077 D4 / D5）：切片 → 逐根 SQL（Initial + 房间边 + UPSERT 回执）→ 按 8 MiB 装笔。
pub(crate) mod model_initial_persist;

pub mod concurrency;

pub mod cata_model;

pub mod prim_model;

pub mod loop_model;

pub mod shared;

#[cfg(feature = "legacy_model")]
pub(crate) mod mesh_generate;

pub mod aabb_refresh;

pub mod libgm_discretise;

pub mod mesh_primitives;

pub mod sweep_mesh;

#[cfg(test)]
pub mod mesh_assert;

pub mod manifold_bool;
pub(crate) mod manifold_types;

#[cfg(feature = "manifold")]
pub mod manifold_csg;

#[cfg(feature = "manifold")]
pub mod manifold_tessellate;

pub mod room_predicate;

pub mod room_model;

pub(crate) mod room_topology;

pub(crate) mod room_resident;

pub mod cal_model;

pub mod pdms_inst;

pub(crate) mod shape_save;

pub mod resolve;

pub mod query;

pub mod utils;

pub mod aabb_tree;

pub mod spatial_state;

pub mod spatial_query;

pub mod spatial_centerline;

pub mod spatial_clearance;

pub mod coverage_audit;

use aios_core::RefU64;
use dashmap::{DashMap, DashSet};
use once_cell::sync::Lazy;
use parry3d::bounding_volume::Aabb;
pub use query::*;
pub use resolve::*;

pub const SEND_INST_SIZE: usize = 500;
pub static EXIST_MESH_GEO_HASHES: Lazy<DashMap<String, Aabb>> = Lazy::new(DashMap::new);
