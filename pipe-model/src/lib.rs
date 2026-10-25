//! pipe-model：E3D 3.1 管道建模语义的 Rust 服务域。
//!
//! 需求规格：`docs/specs/pipe-modeling.md`
//! 行为依据：`docs/evidence/2026-10-15-e3d-pipe-modeling-ui-behavior.md` §15-§18
//! （PML 源码行级伪码 + 托管 PF* 参照）。
//!
//! 设计要点（spec Implementation Decisions）：
//! - 服务唯一产出物是 [`MutationBatch`]——与 `e3d_collector` 落库行同构的变更批；
//!   不直接写 SurrealDB，由调用方走 `persist_writer` lane 提交。
//! - `MutationBatch` 即原子/撤销边界（E3D `UNDOABLE` 等价物），批内任一失败整批不提交。
//! - E3D 的静默降级路径（CONNECT 几何兜底、FCONN+minTube 占位、规格回落）在本服务
//!   全部改为显式返回值（见 [`ConnectOutcome`] / [`PipeError`] 分级）。

pub mod batch;
pub mod catalog;
pub mod choice;
pub mod component;
pub mod context;
pub mod error;
pub mod fabrication;
pub mod loader;
pub mod naming;
pub mod outcome;
pub mod pipeline;
pub mod render;
pub mod service;
pub mod settings;
pub mod slope;
pub mod spec_resolver;
pub mod specchange;
pub mod splitmerge;
pub mod tapping;
#[cfg(test)]
pub mod testkit;
pub mod weld;

pub use batch::{Confirm, MutationBatch, MutationOp};
pub use catalog::PipeCatalog;
pub use choice::QuestionAnswers;
pub use context::{BranchEnd, Context, Flow};
pub use error::PipeError;
pub use outcome::{ComponentOutcome, ConnectOutcome, FabMessage, FabStatus, PipeWarning};
pub use service::{PipeModel, PipeModelService};
pub use settings::PipingSettings;
pub use spec_resolver::SpecCatalog;
