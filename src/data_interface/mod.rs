pub mod db_model;
// pub mod spatial_model;
pub mod interface;
pub mod structs;
pub mod tidb_manager;

pub mod mesh_manager;

pub mod db_manager;

pub mod project_paths;

pub mod extract_family;

pub mod debug_scope;

pub mod watch_scope;

pub mod increment_manager;

pub mod sweep_log;

pub mod increment_pipeline;

pub mod window_repair;

pub mod model_refresh;

pub mod model_impact;

pub mod sesno_range;

// 净窗口收集（`net_window`）与会话索引差分（`session_index_diff`）已下沉到
// pdms-io（2026-08-19）：它们的输入是「库文件 + sesno 窗口」，产出是 pdms-io 自己
// 的操作流类型，中间不碰库——与被它替代的 legacy 逐会话回放同层。上层按
// `pdms_io::net_window` / `pdms_io::session_index_diff` 直接引用，本模块不做转发。

pub mod dbnum_state;

pub mod fast_delete;

pub mod manual_update;

pub mod update_scope;

pub mod on_demand_model;

pub(crate) mod on_demand_db;

pub mod sync_publisher;

pub mod side_effect_pending;

pub mod helper;

pub mod cata_closure;

pub mod parse_error;

pub mod geom_error;

pub mod embedded_surql;

pub mod generation_root;

pub mod model_update_plan;

pub mod model_update_pending;

pub mod model_concurrency;

pub mod model_rebuild;

pub mod batch_queue;

pub mod batch_scheduler;

pub mod initialization_phase;

pub mod batch_worker;

pub mod queue_stall_diagnostics;

pub mod task_registry;

pub mod staging;
