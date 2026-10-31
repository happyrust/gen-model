pub mod e3d_mesh_store;
pub mod e3d_model_service;
/// 生成器身份（GitHub #73）：逐根回执的 `generator_fingerprint` 与水位行库级落款 `model_generator` 共用的那一串。
pub mod generator_identity;
#[cfg(feature = "legacy_model")]
pub(crate) mod gen_model;
pub mod historical_model;
/// 最小交付单元的模型版本表：从会话链按需算、不落库（ADR-081）。
pub mod unit_versions;
/// 单个构件的模型版本表：同一条链、只认它自己，并排给所属单元那一列。
pub mod element_versions;
/// 一个节点的属性变化时间线：谁在哪一版改了什么（会话 user / comment + 逐属性 before / after）。
pub mod attribute_history;
/// 一个节点两版之间的属性净差：A / B 各钉一个会话、同一个渲染器两端各出一次字，不折时间线、不要快照。
pub mod attribute_diff;
/// 节点两版之间的差异摘要：索引差分 + 模型影响，按最小交付单元分组，不生成几何。
pub mod node_diff_summary;
/// 任意节点按对比范围（自身 / 整棵子树）折叠的模型版本表：容器节点的时间线靠它，每版附动了几个单元。
pub mod node_versions;
pub(crate) mod model_artifact;
pub mod model_db_adapter;
pub mod model_memory_store;
pub mod model_record_reader;
pub(crate) mod model_write_behind;
/// 初始化直写的模型侧渲染（ADR-077 D4 / D5）：切片 → 逐根 SQL（Initial + 房间边 + UPSERT 回执）→ 按 8 MiB 装笔。
pub(crate) mod model_initial_persist;
/// `pe_ptset` 分表：管件目录 P 点随根发布的行形状与渲染（ptset 分表计划 W2）。
pub(crate) mod ptset_rows;

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
pub mod spatial_branches;
/// 每根 BRAN 的直管段并集盒（`nearby/branches` 补没加载过的邻管）。
pub mod spatial_tube_index;

pub mod spatial_discipline;

pub mod spatial_rooms;
/// 房间层级树（plant3d-web ADR 0068）：`nearby/tree` 与 `rooms/{refno}/tree` 的聚合。
pub mod spatial_tree_view;
/// 房间层级树 `tubes=1` 的直段来源（方案 B）：按 BRAN 单元从投影 / `tubi_relate` 取隐式管身记录。
pub mod spatial_tree_tubes;

pub mod spatial_centerline;

pub mod spatial_clearance;

pub mod surface_clearance;

pub mod coverage_audit;

use aios_core::RefU64;
use dashmap::{DashMap, DashSet};
use once_cell::sync::Lazy;
use parry3d::bounding_volume::Aabb;
pub use query::*;
pub use resolve::*;

pub const SEND_INST_SIZE: usize = 500;
pub static EXIST_MESH_GEO_HASHES: Lazy<DashMap<String, Aabb>> = Lazy::new(DashMap::new);
