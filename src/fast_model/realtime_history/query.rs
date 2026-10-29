//! 第 6–8 步：实时历史几何查询的编排（design §1.2 第 3–8 步）。
//!
//! 一次请求的固定顺序（前一步失败不再做后一步，Requirement 6.16）：
//!
//! 1. 会话解析：`sesno` 选择器校验它在会话链上；`time` 选择器在该单元「仅自身」版本表上
//!    按链序规则（[`resolve_parsed_at_rows`](crate::fast_model::time_resolve::resolve_parsed_at_rows)）
//!    解析成 `(sesno, session_time)`。两条路都要那张版本表，所以先取它一次：`unit_noun` 用来判
//!    `NOT_A_DELIVERY_UNIT_ROOT`，行集用来在根不在索引时判 [`classify_absent`]。
//! 2. 源文件集指纹 → 结果缓存。命中不占并发名额；`If-None-Match` 含命中的 `geometryKey` 回
//!    `NotModified`，否则回 `Body`（Requirement 7.3）。
//! 3. 并发闸：满则 `Busy`（503 + `Retry-After`），不计命中 / 未命中（7.1）。
//! 4. `spawn_blocking` 跑 [`RealtimeCalculator::compute`]，permit 随闭包进阻塞线程，计算真停下
//!    才归还名额。
//! 5. 超时（`model_history_query_timeout_secs`）：置位 [`CancelToken`] 并回 `Timeout`；阻塞线程
//!    在下一个检查点退出（Requirement 6.9）。
//! 6. 只写 200 进缓存，再按 `If-None-Match` 决定 304 / 200（Requirement 6.17、7.2、7.7）。
//!
//! 整条路不建任务、不碰 [`HistoricalModelStore`](crate::fast_model::historical_model::HistoricalModelStore)、
//! 不写投影存储（Requirement 6.12）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::fast_model::cancel::{CancelToken, HistoryHooks};
use crate::fast_model::historical_model::{
    HistoricalModelError, MAX_HISTORY_ELEMENTS, instance_json, source_service, tube_json,
};
use crate::fast_model::realtime_history::absent::{AbsentKind, classify_absent};
use crate::fast_model::realtime_history::cache::{
    CachedHistoryResponse, RealtimeCacheKey, RealtimeResultCache,
};
use crate::fast_model::realtime_history::digest::{geometry_digest, source_set_fingerprint};
use crate::fast_model::realtime_history::gate::{GateBusy, RealtimeGate};
use crate::fast_model::realtime_history::validate::{
    RealtimeSelector, RequestError, ValidatedRequest, validate_request,
};
use crate::fast_model::time_resolve::{ResolveError, VersionInstant, resolve_parsed_at_rows};
use crate::fast_model::unit_versions::{UnitVersionsError, ensure_delivery_unit, unit_versions};

/// 一次实时历史几何请求（design §1.2 的 `RealtimeHistoryRequest`）。
#[derive(Debug, Clone)]
pub struct RealtimeHistoryRequest {
    /// handler 原样收下的查询参数，交给 [`validate_request`] 裁决。
    pub params: crate::fast_model::realtime_history::validate::RealtimeQueryParams,
    /// 已去引号、去 `W/` 前缀的 `If-None-Match` 值。
    pub if_none_match: Vec<String>,
}

/// 编排的三种结局。
#[derive(Debug)]
pub enum RealtimeOutcome {
    /// 200：已序列化的体与头部。
    Body(Arc<CachedHistoryResponse>),
    /// 304：摘要相同，调用方复用已装入的那份几何。
    NotModified {
        sesno: u32,
        session_time: Option<String>,
        geometry_key: String,
    },
    /// 503：并发闸满，`retry_after_secs` 给 `Retry-After`。
    Busy(GateBusy),
}

/// 编排失败。HTTP 状态、错误码、`detail` 在这里定好，handler 只做搬运。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
    pub detail: Option<Value>,
}

impl RealtimeError {
    fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            detail: None,
        }
    }

    fn with_detail(mut self, detail: Value) -> Self {
        self.detail = Some(detail);
        self
    }

    /// 请求被取消（客户端断开或超时）：handler 通常已经丢弃了 future，走到这里说明
    /// 取消是在返回路径上才被观察到的。回 504 与超时同码，调用方不必分辨。
    fn cancelled() -> Self {
        Self::new(504, "HISTORY_QUERY_TIMEOUT", "query cancelled before it finished")
    }

    fn timed_out(secs: u64) -> Self {
        Self::new(
            504,
            "HISTORY_QUERY_TIMEOUT",
            format!("query exceeded {secs} seconds"),
        )
    }
}

impl From<RequestError> for RealtimeError {
    fn from(error: RequestError) -> Self {
        Self::new(
            error.status() as u16,
            error.code(),
            error.message().to_string(),
        )
        .with_detail(error.detail())
    }
}

impl From<UnitVersionsError> for RealtimeError {
    fn from(error: UnitVersionsError) -> Self {
        let status = match error.code {
            "INVALID_REFNO" | "INVALID_VERSION_PAIR" => 400,
            "REFNO_NOT_FOUND" => 404,
            "NOT_A_DELIVERY_UNIT_ROOT" => 422,
            "COMPUTATION_TIMEOUT" => 504,
            "SOURCE_NOT_FOUND" | "SESSION_TIME_UNAVAILABLE" => 503,
            _ => 500,
        };
        let mapped = Self::new(status, error.code, error.message);
        match error.detail {
            Some(detail) => mapped.with_detail(detail),
            None => mapped,
        }
    }
}

impl From<HistoricalModelError> for RealtimeError {
    fn from(error: HistoricalModelError) -> Self {
        let status = match error.code {
            "INVALID_SELECTOR" | "INVALID_REFNO" => 400,
            "SESSION_NOT_FOUND" | "REFNO_NOT_FOUND_AT_SESSION" => 404,
            "TIME_BEFORE_OLDEST_SESSION" | "NO_RESOLVABLE_SESSION" => 422,
            "GENERATION_LIMIT_EXCEEDED" => 413,
            "GENERATION_CANCELLED" => 504,
            "SESSION_TIME_UNAVAILABLE" | "SOURCE_NOT_FOUND" => 503,
            _ => 500,
        };
        Self::new(status, error.code, error.message)
    }
}

impl std::fmt::Display for RealtimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RealtimeError {}

/// 一次计算产出的几何行（未序列化）。
#[derive(Debug, Clone, Default)]
pub struct ComputedGeometry {
    /// `instances` 行，已按 `(source_refno, id)` 排序、按 `id` 去重留最后、`snapshot_key` 已删。
    pub instances: Vec<Value>,
    /// `tubes` 行，同上。
    pub tubes: Vec<Value>,
    pub visited: usize,
    pub generated: usize,
    pub failed: usize,
}

/// 实时几何的计算器。真实现包 [`E3dModelService::realtime_compute_source`] 与
/// [`project_historical_records`]；测试注入假实现（Requirement 6.6–6.8）。
///
/// 实现必须是可跨线程的：编排在 `spawn_blocking` 里调它。
pub trait RealtimeCalculator: Send + Sync {
    /// 同步阻塞：冷输入要扫索引、建 `DbSet`，求值要三角化。
    ///
    /// 取消时返回 [`RealtimeError`] 的 504；超元素上限回 413；根不在索引回 404 并带
    /// `detail.reason`（`not-yet` / `deleted`）。
    fn compute(
        &self,
        dbnum: u32,
        unit: e3d_io::refno::RefNo,
        sesno: u32,
        cancel: &CancelToken,
    ) -> Result<ComputedGeometry, RealtimeError>;
}

/// 生产实现：与 Legacy 快照路共用 `realtime_compute_source` + `project_historical_records`。
///
/// 网格写入不加锁：`ensure_mesh_file` 是内容寻址 + 同目录临时文件 + `hard_link` 原子发布，
/// 与生成并发写同一 `mesh_id` 也安全（`docs/verification/model-version-timeline-scrub/phase1-mesh-write-safety.md`）。
pub struct ServiceCalculator;

impl RealtimeCalculator for ServiceCalculator {
    fn compute(
        &self,
        dbnum: u32,
        unit: e3d_io::refno::RefNo,
        sesno: u32,
        cancel: &CancelToken,
    ) -> Result<ComputedGeometry, RealtimeError> {
        let started = Instant::now();
        let service = source_service()
            .map_err(|error| RealtimeError::new(503, "SOURCE_NOT_FOUND", format!("{error:#}")))?;
        let mesh_dir = crate::options::meshes_dir_for(&aios_core::get_db_option());
        let hooks = HistoryHooks::new(cancel.clone(), MAX_HISTORY_ELEMENTS);
        let snapshot = match service.realtime_compute_source(
            dbnum,
            unit,
            sesno,
            crate::data_interface::geom_error::GeometryFailurePolicy::BestEffortFallback,
            &hooks,
        ) {
            Ok(snapshot) => snapshot,
            Err(error) => return Err(compute_error(error, dbnum, unit, sesno)),
        };

        let projection = crate::fast_model::e3d_model_service::project_historical_records(
            dbnum,
            sesno,
            snapshot.elements,
            &snapshot.owners,
            &mesh_dir,
            Some(cancel.flag()),
        );
        let projection = match projection {
            Ok(projection) => projection,
            Err(error) => {
                if error
                    .downcast_ref::<crate::fast_model::e3d_model_service::HistoricalProjectionCancelled>()
                    .is_some()
                {
                    return Err(RealtimeError::cancelled());
                }
                return Err(RealtimeError::new(
                    500,
                    "GEOMETRY_GENERATION_FAILED",
                    format!("{error:#}"),
                ));
            }
        };

        // 去重（同一 `record_id` 后写覆盖先写，与投影存储的语义一致）→ 按 (source_refno, id) 排序
        // → 序列化并删 `snapshot_key`。
        let mut instances: Vec<Value> = Vec::new();
        let mut tubes: Vec<Value> = Vec::new();
        for record in projection.records() {
            let is_tube = record.tube.is_some();
            let mut row = if is_tube {
                tube_json(record, "")
            } else {
                instance_json(record, "")
            };
            if let Value::Object(map) = &mut row {
                map.remove("snapshot_key");
            }
            if is_tube {
                tubes.push(row);
            } else {
                instances.push(row);
            }
        }
        sort_and_dedup(&mut instances);
        sort_and_dedup(&mut tubes);

        let _elapsed = started.elapsed();
        Ok(ComputedGeometry {
            instances,
            tubes,
            visited: snapshot.report.visited,
            generated: snapshot.report.generated,
            failed: snapshot.report.failed.len(),
        })
    }
}

fn compute_error(
    error: crate::fast_model::e3d_model_service::RealtimeComputeError,
    dbnum: u32,
    unit: e3d_io::refno::RefNo,
    sesno: u32,
) -> RealtimeError {
    use crate::fast_model::e3d_model_service::{RealtimeComputeError, SnapshotSourceError};
    match error {
        RealtimeComputeError::Stopped { stop, .. } => match stop {
            e3d_model::pipeline::Stop::Cancelled => RealtimeError::cancelled(),
            e3d_model::pipeline::Stop::LimitExceeded => RealtimeError::new(
                413,
                "GENERATION_LIMIT_EXCEEDED",
                format!("subtree exceeds {MAX_HISTORY_ELEMENTS} elements"),
            ),
        },
        RealtimeComputeError::Failed(error) => {
            if matches!(
                error.downcast_ref::<SnapshotSourceError>(),
                Some(SnapshotSourceError::RootNotInSession { .. })
            ) {
                absent_error(dbnum, unit, sesno)
            } else {
                RealtimeError::new(
                    500,
                    "GEOMETRY_GENERATION_FAILED",
                    format!("{error:#}"),
                )
            }
        }
    }
}

/// 根不在该会话索引时，取单元「仅自身」版本表判原因（Requirement 6.5）。
fn absent_error(dbnum: u32, unit: e3d_io::refno::RefNo, sesno: u32) -> RealtimeError {
    let rows = match unit_version_rows(dbnum, unit) {
        Ok(rows) => rows,
        Err(error) => return error,
    };
    let kind = classify_absent(&rows.rows, sesno);
    let mapped = RealtimeError::new(
        kind.status() as u16,
        kind.code(),
        kind.message(sesno),
    );
    match kind.reason() {
        Some(reason) => mapped.with_detail(json!({ "reason": reason.as_str() })),
        None => mapped,
    }
}

/// 一张单元的「仅自身」版本表（链序）与根 noun。
struct UnitVersionRows {
    rows: Vec<crate::fast_model::unit_versions::UnitVersion>,
    noun: String,
}

/// 取全表：`unit_versions` 一次最多 5000 行，截断时按 `since_sesno` 翻页续取。
fn unit_version_rows(
    dbnum: u32,
    unit: e3d_io::refno::RefNo,
) -> Result<UnitVersionRows, RealtimeError> {
    const PAGE: usize = 5000;
    let mut all = Vec::new();
    let mut since: Option<u32> = None;
    let mut noun: Option<String> = None;
    loop {
        let report = unit_versions(dbnum, unit, since, Some(PAGE))?;
        if noun.is_none() {
            noun = Some(report.unit_noun.clone());
        }
        let last = report.versions.last().map(|row| row.sesno);
        let truncated = report.truncated;
        all.extend(report.versions);
        match (truncated, last) {
            (true, Some(sesno)) => since = Some(sesno),
            _ => break,
        }
    }
    Ok(UnitVersionRows {
        rows: all,
        noun: noun.unwrap_or_default(),
    })
}

/// 按 `(source_refno, id)` 排序，同 `id` 只留**最后**一条（与投影存储「后写覆盖先写」的语义一致）。
///
/// `Vec::dedup_by` 留的是先出现的那条，所以从后往前扫一遍记已见 id 再翻转回来：排序是稳定的，
/// 同 id 的相对顺序仍是产出顺序，翻转后第一条就是原来最后写入的那条。
fn sort_and_dedup(rows: &mut Vec<Value>) {
    let key_of = |row: &Value| {
        (
            row.get("source_refno")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            row.get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        )
    };
    rows.sort_by(|left, right| key_of(left).cmp(&key_of(right)));

    let mut seen = std::collections::HashSet::with_capacity(rows.len());
    let mut kept: Vec<Value> = Vec::with_capacity(rows.len());
    for row in rows.drain(..).rev() {
        let id = key_of(&row).1;
        if seen.insert(id) {
            kept.push(row);
        }
    }
    kept.reverse();
    *rows = kept;
}

/// 编排状态：闸、结果缓存、超时与计算器。
///
/// 生产走 [`RealtimeHistoryState::global`]（各项取配置），测试用 [`RealtimeHistoryState::new`]
/// 注入假计算器与可控时钟。
pub struct RealtimeHistoryState {
    gate: RealtimeGate,
    cache: Arc<RealtimeResultCache>,
    timeout: Duration,
    calculator: Arc<dyn RealtimeCalculator>,
}

impl RealtimeHistoryState {
    pub fn new(
        gate: RealtimeGate,
        cache: Arc<RealtimeResultCache>,
        timeout: Duration,
        calculator: Arc<dyn RealtimeCalculator>,
    ) -> Self {
        Self {
            gate,
            cache,
            timeout,
            calculator,
        }
    }

    /// 进程内共享的那一份（路由与 `/health` 共用同一道闸、同一张缓存）。
    pub fn global() -> &'static RealtimeHistoryState {
        static STATE: std::sync::OnceLock<RealtimeHistoryState> = std::sync::OnceLock::new();
        STATE.get_or_init(|| {
            RealtimeHistoryState::new(
                RealtimeGate::global().clone(),
                Arc::new(RealtimeResultCache::with_configured_capacity()),
                Duration::from_secs(crate::options::model_history_query_timeout_secs() as u64),
                Arc::new(ServiceCalculator),
            )
        })
    }

    pub fn gate(&self) -> &RealtimeGate {
        &self.gate
    }

    pub fn cache(&self) -> &RealtimeResultCache {
        &self.cache
    }

    pub fn timeout_secs(&self) -> u64 {
        self.timeout.as_secs()
    }
}

/// 入口：`GET /api/v1/model/history/geometry` 的编排。
pub async fn query_realtime(
    req: RealtimeHistoryRequest,
    cancel: CancelToken,
) -> Result<RealtimeOutcome, RealtimeError> {
    query_realtime_with(RealtimeHistoryState::global(), req, cancel).await
}

/// [`query_realtime`] 的可注入版本（测试用同一段编排，只换状态）。
pub async fn query_realtime_with(
    state: &RealtimeHistoryState,
    req: RealtimeHistoryRequest,
    cancel: CancelToken,
) -> Result<RealtimeOutcome, RealtimeError> {
    let validated: ValidatedRequest = validate_request(&req.params)?;
    let dbnum = validated.dbnum;
    let unit = validated.unit;

    // 第 3 步：会话解析。两条路都要那张版本表（noun 检查与 absent 判定），先取一次。
    let table = unit_version_rows(dbnum, unit)?;
    ensure_delivery_unit(&table.noun, &configured_unit_types(), unit)?;
    let (sesno, session_time) = resolve_session(&table, &validated.selector)?;

    // 第 4 步：指纹与缓存。
    let service = source_service()
        .map_err(|error| RealtimeError::new(503, "SOURCE_NOT_FOUND", format!("{error:#}")))?;
    let source_file = service
        .source_file(dbnum)
        .map_err(|error| RealtimeError::new(503, "SOURCE_NOT_FOUND", format!("{error:#}")))?;
    let fingerprint = source_set_fingerprint(&source_file).map_err(|error| {
        RealtimeError::new(503, "SOURCE_NOT_FOUND", format!("{error:#}"))
    })?;
    let key = RealtimeCacheKey {
        dbnum,
        unit: (unit.word0, unit.word1),
        sesno,
        fingerprint: fingerprint.clone(),
    };
    if let Some(cached) = state.cache.get(&key) {
        return Ok(if matches_key(&req.if_none_match, &cached.geometry_key) {
            RealtimeOutcome::NotModified {
                sesno: cached.sesno,
                session_time: cached.session_time.clone(),
                geometry_key: cached.geometry_key.clone(),
            }
        } else {
            RealtimeOutcome::Body(cached)
        });
    }

    // 第 5 步：并发闸。
    let permit = match state.gate.try_acquire() {
        Ok(permit) => permit,
        Err(busy) => return Ok(RealtimeOutcome::Busy(busy)),
    };
    state.cache.record_miss();

    // 第 6、7 步：阻塞计算 + 超时。
    let calculator = Arc::clone(&state.calculator);
    let worker_cancel = cancel.clone();
    let started = Instant::now();
    let handle = tokio::task::spawn_blocking(move || {
        // permit 随闭包进阻塞线程：计算真停下才归还名额。
        let _permit = permit;
        calculator.compute(dbnum, unit, sesno, &worker_cancel)
    });
    let joined = match tokio::time::timeout(state.timeout, handle).await {
        Ok(joined) => joined,
        Err(_) => {
            // 超时：置位取消，阻塞线程在下一个检查点退出并释放名额。
            cancel.cancel();
            return Err(RealtimeError::timed_out(state.timeout.as_secs()));
        }
    };
    let computed = joined.map_err(|error| {
        RealtimeError::new(
            500,
            "internal",
            format!("实时历史计算线程异常退出: {error}"),
        )
    })??;
    if cancel.is_cancelled() {
        return Err(RealtimeError::cancelled());
    }

    // 第 8 步：序列化 → 只写 200 → 按 If-None-Match 决定 304 / 200。
    let geometry_key = geometry_digest(&computed.instances, &computed.tubes);
    let body = serde_json::to_vec(&json!({
        "dbnum": dbnum,
        "unit_refno": format!("{}_{}", unit.word0, unit.word1),
        "sesno": sesno,
        "session_time": session_time,
        "geometryKey": geometry_key,
        "instances": computed.instances,
        "tubes": computed.tubes,
        "stats": {
            "visited": computed.visited,
            "generated": computed.generated,
            "failed": computed.failed,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        },
    }))
    .map_err(|error| RealtimeError::new(500, "internal", format!("响应序列化失败: {error}")))?;
    let cached = Arc::new(CachedHistoryResponse::new(
        sesno,
        session_time.clone(),
        geometry_key.clone(),
        body,
    ));
    state.cache.insert(key, 200, Arc::clone(&cached));
    Ok(if matches_key(&req.if_none_match, &geometry_key) {
        RealtimeOutcome::NotModified {
            sesno,
            session_time,
            geometry_key,
        }
    } else {
        RealtimeOutcome::Body(cached)
    })
}

fn configured_unit_types() -> Vec<String> {
    crate::data_interface::generation_root::configured_delivery_unit_types()
}

/// `If-None-Match` 是否命中这个 `geometryKey`（已去引号、去 `W/`）。
fn matches_key(candidates: &[String], geometry_key: &str) -> bool {
    candidates
        .iter()
        .any(|candidate| candidate == geometry_key || candidate == "*")
}

/// 第 3 步：把选择器解析成 `(sesno, session_time)`。
fn resolve_session(
    table: &UnitVersionRows,
    selector: &RealtimeSelector,
) -> Result<(u32, Option<String>), RealtimeError> {
    match selector {
        RealtimeSelector::Sesno(sesno) => {
            let row = table
                .rows
                .iter()
                .find(|row| row.sesno == *sesno)
                .ok_or_else(|| {
                    RealtimeError::new(
                        404,
                        "SESSION_NOT_FOUND",
                        format!("会话 {sesno} 不在该单元的版本表上"),
                    )
                })?;
            Ok((row.sesno, row.session_time.clone()))
        }
        RealtimeSelector::At(parsed) => {
            let rows: Vec<VersionInstant> = table
                .rows
                .iter()
                .map(|row| VersionInstant {
                    sesno: row.sesno,
                    session_time: row.session_time.clone(),
                })
                .collect();
            match resolve_parsed_at_rows(&rows, *parsed) {
                Ok((sesno, time)) => Ok((sesno, Some(time))),
                Err(ResolveError::InvalidFormat) => Err(RealtimeError::new(
                    400,
                    "INVALID_SELECTOR",
                    "time 不符合 YYYY-MM-DD[T| ]HH:mm[:ss[.fff]][Z|±HH:MM]",
                )),
                Err(ResolveError::BeforeOldest) => Err(RealtimeError::new(
                    422,
                    "TIME_BEFORE_OLDEST_SESSION",
                    "时间点早于该单元全部带时间的会话",
                )),
                Err(ResolveError::NoResolvable) => Err(RealtimeError::new(
                    422,
                    "NO_RESOLVABLE_SESSION",
                    "该单元的版本表里没有任何带 Session_Time 的行",
                )),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::fast_model::realtime_history::validate::RealtimeQueryParams;

    /// 假计算器：按 `(sesno)` 返回固定行，可注入延迟、取消与错误。
    struct FakeCalculator {
        calls: AtomicUsize,
        rows: Mutex<Vec<(u32, usize)>>,
        fail: Mutex<Option<RealtimeError>>,
    }

    impl FakeCalculator {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                rows: Mutex::new(Vec::new()),
                fail: Mutex::new(None),
            })
        }
    }

    impl RealtimeCalculator for FakeCalculator {
        fn compute(
            &self,
            _dbnum: u32,
            _unit: e3d_io::refno::RefNo,
            sesno: u32,
            cancel: &CancelToken,
        ) -> Result<ComputedGeometry, RealtimeError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if let Some(error) = self.fail.lock().unwrap().clone() {
                return Err(error);
            }
            if cancel.is_cancelled() {
                return Err(RealtimeError::cancelled());
            }
            Ok(ComputedGeometry {
                instances: vec![json!({ "id": format!("i-{sesno}"), "source_refno": "1_2" })],
                tubes: Vec::new(),
                visited: 3,
                generated: 1,
                failed: 0,
            })
        }
    }

    /// 把版本表与源文件都换掉的注入点：这里只验证编排顺序，不碰真库。
    #[test]
    fn dedup_keeps_the_last_row_for_the_same_id() {
        let mut rows = vec![
            json!({ "id": "b", "source_refno": "1_2" }),
            json!({ "id": "a", "source_refno": "1_2" }),
            json!({ "id": "a", "source_refno": "1_2", "marker": "last" }),
        ];
        sort_and_dedup(&mut rows);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["id"], "a");
        assert_eq!(rows[0]["marker"], "last");
        assert_eq!(rows[1]["id"], "b");
    }

    #[test]
    fn if_none_match_accepts_exact_key_and_star() {
        assert!(matches_key(&["g:abc".into()], "g:abc"));
        assert!(matches_key(&["*".into()], "g:abc"));
        assert!(!matches_key(&["g:abd".into()], "g:abc"));
        assert!(!matches_key(&[], "g:abc"));
    }

    #[test]
    fn validation_errors_carry_status_code_and_detail() {
        let params = RealtimeQueryParams {
            dbnum: Some("8000".into()),
            unit_refno: Some("24384_23257".into()),
            sesno: Some("1".into()),
            time: Some("2026-01-01T00:00".into()),
        };
        let error: RealtimeError = validate_request(&params).unwrap_err().into();
        assert_eq!(error.status, 400);
        assert_eq!(error.code, "INVALID_SELECTOR");
        assert_eq!(error.detail, Some(json!({ "params": ["sesno", "time"] })));
    }

    #[test]
    fn absent_kind_maps_to_status_and_reason_detail() {
        use crate::fast_model::unit_versions::{UnitVersion, UnitVersionImpact};
        let rows = vec![UnitVersion {
            sesno: 10,
            session_time: None,
            impact_kind: UnitVersionImpact::Tombstone,
        }];
        let kind = classify_absent(&rows, 20);
        assert_eq!(kind.status(), 404);
        assert_eq!(kind.reason().map(|reason| reason.as_str()), Some("deleted"));
    }

    #[test]
    fn health_section_reports_cache_gate_and_input_cache() {
        // 只验证形状与取值来源：全局状态的读数与配置一致，且字段名与 design §1.8 对齐。
        let state = RealtimeHistoryState::global();
        let stats = state.cache().stats();
        assert_eq!(stats.capacity, crate::options::model_history_realtime_cache());
        assert_eq!(state.gate().limit(), crate::options::model_history_realtime_concurrency());
        assert_eq!(
            state.timeout_secs(),
            crate::options::model_history_query_timeout_secs() as u64
        );
        // 没有在跑的计算时，闸的读数就是 0；并发上限与配置一致。
        assert!(state.gate().in_flight() <= state.gate().limit());
    }

    #[test]
    fn fake_calculator_counts_calls() {
        let fake = FakeCalculator::new();
        let cancel = CancelToken::new();
        let computed = fake
            .compute(8000, e3d_io::refno::RefNo::new(1, 2), 7, &cancel)
            .expect("computed");
        assert_eq!(computed.visited, 3);
        assert_eq!(fake.calls.load(Ordering::Relaxed), 1);
    }
}
