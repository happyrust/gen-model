//! 实时历史几何查询（design §1.2）：`GET /api/v1/model/history/geometry`。
//!
//! 只读、同步：请求内由 e3d-model 按会话链算出单元在该会话下的几何，同一个响应返回
//! `instances` 与 `tubes`；不建任务、不写投影存储。各步纯逻辑分在子模块里，编排在 [`query`]。

/// 第 1、2 步：字段与选择器校验（Requirement 6.2、6.6、6.16）。
pub mod validate;

/// 根不在索引时判定单元不存在的原因（Requirement 6.5）。
pub mod absent;

pub use absent::{AbsentKind, AbsentReason, classify_absent};

/// 几何摘要 `geometryKey` 与源文件集指纹（Requirement 6.10、6.11、7.4）。
pub mod digest;

pub use digest::{geometry_digest, source_set_fingerprint};

pub use validate::{
    RealtimeQueryParams, RealtimeSelector, RequestError, ValidatedRequest, validate_request,
};

/// 第 5 步：实时计算并发闸（Requirement 6.17、7.1、7.5）。
pub mod gate;

pub use gate::{GateBusy, RealtimeGate, retry_after_secs};

/// 第 4、8 步：结果缓存（Requirement 6.17、7.2–7.5）。
pub mod cache;

pub use cache::{CachedHistoryResponse, RealtimeCacheKey, RealtimeCacheStats, RealtimeResultCache};

/// 第 3–8 步：会话解析、缓存、并发闸、阻塞计算、超时与写缓存的编排。
pub mod query;

pub use query::{
    ComputedGeometry, RealtimeCalculator, RealtimeError, RealtimeHistoryRequest,
    RealtimeHistoryState, RealtimeOutcome, ServiceCalculator, query_realtime, query_realtime_with,
    resolve_error_code, resolve_error_status,
};
