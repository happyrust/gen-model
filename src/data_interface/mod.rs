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
#[cfg(test)]
mod issue10_direct_add_node;

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

pub mod mdb_membership;

pub mod update_scope;

pub mod on_demand_model;

pub(crate) mod on_demand_db;

pub mod sync_publisher;

pub mod side_effect_pending;

pub mod helper;

pub mod cata_closure;

/// ADR-053 direct 模式的取数底座：按 dbnum 池化 e3d-io 引擎、钉在各库
/// `applied_sesno` 上，直接从 db 文件取元素属性，不经 SurrealDB。
pub mod direct_store;
pub mod direct_tree;
pub mod model_source;

/// e3d-io 的 `ElementExtraction` → 生成链消费的 `NamedAttrMap`（ADR-053 Q4）。
pub mod direct_attmap;

/// ADR-053 D3：type / name / backref 三个派生索引，一次全树扫描预建，
/// 磁盘缓存 + 指纹失效。
pub mod direct_index;

pub mod parse_error;

pub mod geom_error;

pub mod embedded_surql;

pub mod generation_root;

/// Core3D 粒度/去重规则的可执行参考模型。**不在生产路径上**——它是给
/// `generation_root` / `model_update_plan` 当契约的，见模块头。
pub mod core3d_reference;

pub mod model_update_plan;

/// 稳态增量窗口 S→T 的模型面选根：`roots_S ∪ roots_T` 与 `touches_roots`（ADR-056 P2-1 / D9 / N7）。
pub mod window_root_plan;

pub mod model_update_pending;

pub mod model_concurrency;

pub mod model_rebuild;

pub mod batch_queue;

pub mod batch_scheduler;

pub mod initialization_phase;

pub mod batch_worker;

pub mod queue_stall_diagnostics;

pub mod batch_failure_log;

pub mod task_registry;

/// 持久层逐表对拍与「mem:// 实例 + 生产 schema」的中性载体（与暂存无关，P3 不删）。
pub mod table_parity;

/// 直写版崩溃重放对拍（ADR-056 实施约束 3 / spec 035 T171）：窗口语句批中途中止 →
/// 同一固定区间重放 → 与一次成功逐表一致。
#[cfg(test)]
mod direct_window_replay_parity;

pub mod staging;
