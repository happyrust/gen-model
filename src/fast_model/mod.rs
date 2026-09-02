pub mod e3d_mesh_store;
pub mod e3d_model_service;
#[cfg(feature = "legacy_model")]
pub(crate) mod gen_model;
pub mod historical_model;

pub mod concurrency;

pub mod cata_model;

pub mod prim_model;

pub mod loop_model;

pub mod shared;

#[cfg(feature = "legacy_model")]
pub(crate) mod occ_generate;

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

pub mod room_fixture;

pub mod room_live_issue7;

pub mod room_predicate;

pub mod room_model;

pub(crate) mod room_publication;

pub(crate) mod room_topology;

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
#[cfg(feature = "legacy_model")]
pub mod legacy {
    use aios_core::options::DbOption;

    pub async fn generate_dbnums(dbnums: &[u32], db_option: &DbOption) -> anyhow::Result<()> {
        super::gen_model::process_meshes_by_dbnos(dbnums, db_option).await
    }
}
use once_cell::sync::Lazy;
use parry3d::bounding_volume::Aabb;
pub use query::*;
pub use resolve::*;

pub const SEND_INST_SIZE: usize = 500;
pub static EXIST_MESH_GEO_HASHES: Lazy<DashMap<String, Aabb>> = Lazy::new(DashMap::new);
