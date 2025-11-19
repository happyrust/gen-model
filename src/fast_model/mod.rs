pub mod gen_model;
pub mod gen_model_old;
// pub mod gen_model_refactored;
// pub mod gen_model_impl;

pub mod cata_model;

pub mod prim_model;

pub mod loop_model;

pub mod shared;

pub mod error_macros;
pub use error_macros::ModelErrorKind;

pub mod capture;

pub mod manifold_bool;
pub mod mesh_generate;

pub mod room_model_v2; // 改进版本的房间模型

// Re-export room model v2 functions
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite-index"))]
pub use room_model_v2::{
    IncrementalUpdateResult, RoomBuildStats, build_room_relations_v2,
    rebuild_room_relations_for_rooms, regenerate_room_models_by_keywords,
    update_room_relations_incremental,
};

pub mod cal_model;

pub mod pdms_inst;

pub mod resolve;

pub mod query;
pub mod query_compat;
pub mod query_provider; // 新的统一查询提供者 // 查询兼容层

pub mod utils;

pub mod export_model;
pub use capture::*;
pub use export_model::{export_glb, export_gltf, export_instanced_bundle, model_exporter};
pub mod material_config;
pub mod unit_converter;

pub mod aabb_tree;

pub mod incremental;

#[cfg(feature = "sqlite-index")]
pub mod aabb_cache;
#[cfg(feature = "sqlite-index")]
pub mod session;

pub mod concurrency;

use aios_core::RefU64;
use dashmap::{DashMap, DashSet};
pub use gen_model::*;
pub use gen_model::gen_all_geos_data;
pub use gen_model_old::query_tubi_size;
pub use gen_model_old::*;
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
pub use aios_core::{debug_model, debug_model_debug, debug_model_trace, debug_model_warn};

// 错误日志模式的全局变量
static DEBUG_MODEL_ERRORS_ONLY: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 设置调试模型错误日志模式
pub fn set_debug_model_errors_only(enabled: bool) {
    DEBUG_MODEL_ERRORS_ONLY.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

/// 检查是否启用了调试模型错误日志模式
pub fn is_debug_model_errors_only() -> bool {
    DEBUG_MODEL_ERRORS_ONLY.load(std::sync::atomic::Ordering::Relaxed)
}

/// 智能调试宏：在错误日志模式下只输出错误，否则输出所有调试信息
#[macro_export]
macro_rules! smart_debug_model {
    ($($arg:tt)*) => {{
        if aios_core::is_debug_model_enabled() {
            if $crate::fast_model::is_debug_model_errors_only() {
                // 在错误日志模式下，只输出包含错误关键词的信息
                let message = format!($($arg)*);
                if message.contains("错误") || message.contains("失败") || message.contains("Error") || message.contains("error") || message.contains("ERROR") {
                    println!("{}", message);
                }
            } else {
                // 正常调试模式，输出所有信息
                println!($($arg)*);
            }
        }
    }};
}

/// 智能调试宏：专门用于输出错误信息（在错误日志模式下总是输出）
#[macro_export]
macro_rules! smart_debug_error {
    ($($arg:tt)*) => {{
        if aios_core::is_debug_model_enabled() {
            println!($($arg)*);
        }
    }};
}

/// 智能包装器：包装 debug_model_debug 宏
#[macro_export]
macro_rules! smart_debug_model_debug {
    ($($arg:tt)*) => {{
        if aios_core::is_debug_model_enabled() {
            if $crate::fast_model::is_debug_model_errors_only() {
                // 在错误日志模式下，只输出包含错误关键词的信息
                let message = format!($($arg)*);
                if message.contains("错误") || message.contains("失败") || message.contains("Error") || message.contains("error") || message.contains("ERROR") ||
                   message.contains("Failed") || message.contains("failed") || message.contains("❌") || message.contains("⚠️") {
                    $crate::fast_model::debug_model_debug!($($arg)*);
                }
            } else {
                // 正常调试模式，输出所有信息
                $crate::fast_model::debug_model_debug!($($arg)*);
            }
        }
    }};
}

/// 智能包装器：包装 debug_model_trace 宏
#[macro_export]
macro_rules! smart_debug_model_trace {
    ($($arg:tt)*) => {{
        if aios_core::is_debug_model_enabled() {
            if $crate::fast_model::is_debug_model_errors_only() {
                // 在错误日志模式下，trace 信息通常不包含错误，所以跳过
            } else {
                // 正常调试模式，输出所有信息
                $crate::fast_model::debug_model_trace!($($arg)*);
            }
        }
    }};
}

/// 智能包装器：包装 debug_model_warn 宏
#[macro_export]
macro_rules! smart_debug_model_warn {
    ($($arg:tt)*) => {{
        if aios_core::is_debug_model_enabled() {
            if $crate::fast_model::is_debug_model_errors_only() {
                // 在错误日志模式下，警告信息通常包含错误相关内容，所以输出
                $crate::fast_model::debug_model_warn!($($arg)*);
            } else {
                // 正常调试模式，输出所有信息
                $crate::fast_model::debug_model_warn!($($arg)*);
            }
        }
    }};
}
