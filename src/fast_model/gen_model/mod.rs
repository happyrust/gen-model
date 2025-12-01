// Full Noun 模式模型生成 - 模块化重构版本
//
// 本模块将原先的 2,095 行单文件重构为模块化结构，解决以下问题：
// 1. 文件过大（超出 250 行限制 8.4 倍）
// 2. 代码冗余（90% 重复代码）
// 3. 配置混乱（双重配置机制）
// 4. 并发性能问题

/// E3D 调试宏
#[macro_export]
macro_rules! e3d_dbg {
    ($($arg:tt)*) => {{
        if $crate::fast_model::gen_model::is_e3d_debug_enabled() {
            println!($($arg)*);
        }
    }};
}

// 核心模块
pub mod categorized_refnos;
pub mod config; // 配置管理 (Phase 2)
pub mod context; // 处理上下文
pub mod errors; // 错误类型 (Phase 2)
pub mod models; // 数据模型定义
pub mod noun_collection; // Noun 收集和分类 // 分类 Refno 存储 (Phase 3)

// 处理器模块
pub mod cate_helpers; // Cate 工具函数
pub mod cate_processor; // Cate 处理器
pub mod cate_single; // Cate 单元件处理
pub mod loop_processor; // Loop 处理器
pub mod prim_processor;
pub mod processor; // 通用处理器（消除冗余） // Prim 处理器

// Full Noun 主逻辑 (Phase 3 - 优化版本)
pub mod full_noun_mode;

// 兼容层：保留旧 API
pub mod legacy;

// 非 Full Noun 模式（增量更新、手动 refno、调试模式等）
pub mod non_full_noun;

// 实用工具
pub mod utilities;

// Mesh 处理
pub mod mesh_processing;

// 重新导出常用类型
pub use context::NounProcessContext;
pub use models::{DbModelInstRefnos, NounCategory};
pub use noun_collection::FullNounCollection;
pub use processor::NounProcessor;

// Phase 2: 错误和配置
pub use config::{BatchSize, Concurrency, FullNounConfig};
pub use errors::{FullNounError, Result};

// Phase 3: 优化后的数据结构和主函数
pub use categorized_refnos::{CategorizedRefnos, CategoryStatistics};
pub use full_noun_mode::{gen_full_noun_geos_optimized, validate_sjus_map};

// 重新导出处理函数
pub use cate_processor::process_cate_refno_page;
pub use loop_processor::process_loop_refno_page;
pub use prim_processor::process_prim_refno_page;

// 兼容层：重新导出旧API
pub use legacy::{gen_all_geos_data, gen_full_noun_geos};

// 非 Full Noun 模式导出
pub use non_full_noun::{gen_geos_data, gen_geos_data_by_dbnum};

// 实用工具函数
pub use utilities::{
    is_e3d_debug_enabled, is_e3d_info_enabled, is_e3d_trace_enabled, query_tubi_size,
};

// Mesh 处理函数
pub use mesh_processing::process_meshes_by_dbnos;
