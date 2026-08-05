//! RVM 基准对拍：把 AVEVA E3D 导出的 RVM 当作几何基准，与本仓生成的模型
//! 数据做结构化对拍，给模型生成的正确性提供可复跑、带退出码的判据。
//!
//! 方案见 `docs/2026-08-04_rvm-baseline-verification-plan.md`。
//!
//! 分两步，中间产物是 JSON 快照：
//!   import   RVM/ATT → 快照（本模块）
//!   compare  快照 vs SurrealDB 生成数据 → 三层差异报告
//!
//! 命名注意：本仓已有一个无关的 `src/rvm/`（PDMS 元素遍历），别混淆。

pub mod att;
pub mod compare;
pub mod identity;
pub mod import;
pub mod snapshot;

pub use compare::{CompareOptions, CompareSummary, default_report_path};
pub use import::{ImportOptions, default_snapshot_path, import_and_save, import_rvm};
pub use snapshot::{RvmGeometry, RvmMember, RvmSnapshot, SnapshotMeta};
