pub mod gen_model;
// pub mod gen_model_refactored;
// pub mod gen_model_impl;

pub mod cata_model;

pub mod prim_model;

pub mod loop_model;

pub mod shared;

pub mod error_macros;

pub mod capture;

pub mod manifold_bool;
pub mod mesh_generate;

pub mod room_model;

pub mod cal_model;

pub mod pdms_inst;

pub mod resolve;

pub mod query;
pub mod query_compat;
pub mod query_provider; // 新的统一查询提供者 // 查询兼容层

pub mod utils;

pub mod export_model;
pub use capture::*;
pub use export_model::{
    export_glb, export_gltf, export_instanced_bundle, export_xkt, model_exporter,
};
pub mod material_config;
pub mod unit_converter;

pub mod aabb_tree;

pub mod incremental;

pub mod aabb_cache;
pub mod session;

pub mod concurrency;

use aios_core::RefU64;
use dashmap::{DashMap, DashSet};
pub use gen_model::*;
use once_cell::sync::Lazy;
use parry3d::bounding_volume::Aabb;
// pub use gen_model_refactored::DbModelInstRefnos;
pub use query::*;
pub use resolve::*;

// Re-export mesh generation functions
pub use mesh_generate::{
    booleans_meshes_in_db, gen_inst_meshes, gen_meshes_in_db, process_meshes_update_db,
    process_meshes_update_db_deep, process_meshes_update_db_deep_default,
    update_inst_relate_aabbs_by_refnos,
};

pub const SEND_INST_SIZE: usize = 500;
pub static EXIST_MESH_GEO_HASHES: Lazy<DashMap<String, Aabb>> = Lazy::new(DashMap::new);

// Re-export debug macros from aios_core
pub use aios_core::debug_macros::{is_debug_model_enabled, set_debug_model_enabled};
pub use aios_core::{debug_model, debug_model_debug, debug_model_trace, debug_model_warn};
