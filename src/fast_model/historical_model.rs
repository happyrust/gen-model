//! On-demand historical model projection.
//!
//! Source reads are session-pinned e3d-io reads.  The resulting geometry lands in
//! its own namespace of the in-process projection store
//! ([`crate::fast_model::model_memory_store`]), never in the production
//! projection and never in a database.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::{Instant, UNIX_EPOCH};

use anyhow::Context;
use chrono::{DateTime, FixedOffset, Utc};
use e3d_io::ReadOnlyEngine;
use e3d_io::refno::RefNo;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;

use crate::data_interface::geom_error::GeometryFailurePolicy;
use crate::fast_model::e3d_model_service::{
    E3dModelService, SnapshotSourceError, apply_geometry_delta_on, current_mdb_sources,
};
use crate::fast_model::model_memory_store::{
    ModelMemoryStore, ProjectedGeometry, ProjectedShape, ProjectionScope,
};

pub const MAX_HISTORY_ELEMENTS: usize = 100_000;

#[derive(Debug, Clone)]
pub enum SessionSelector {
    Latest,
    Sesno(u32),
    At(DateTime<FixedOffset>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedSession {
    pub requested: String,
    pub sesno: u32,
    pub session_time: Option<String>,
}

#[derive(Debug, Clone)]
pub struct HistoricalModelRequest {
    pub dbnum: u32,
    pub refno: RefNo,
    pub selector: SessionSelector,
    pub failure_policy: GeometryFailurePolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalModelReport {
    pub snapshot_key: String,
    pub dbnum: u32,
    pub refno: String,
    pub resolved_session: ResolvedSession,
    pub source_file: String,
    pub source_fingerprint: String,
    pub visited: usize,
    pub generated: usize,
    pub failed: usize,
    pub skipped: usize,
    pub upserted: usize,
    pub shared_instances: usize,
    pub baked_instances: usize,
    pub mesh_written: usize,
    pub mesh_reused: usize,
    pub unique_meshes: usize,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoricalModelError {
    pub code: &'static str,
    pub message: String,
}

impl HistoricalModelError {
    fn new(code: &'static str, error: impl std::fmt::Display) -> Self {
        Self {
            code,
            message: error.to_string(),
        }
    }
}

impl std::fmt::Display for HistoricalModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for HistoricalModelError {}

/// 快照回执表。几何本身不在这里，在投影存储的 `Historical(<key>)` 命名空间里。
#[derive(Default)]
pub struct HistoricalModelStore {
    reports: RwLock<HashMap<String, HistoricalModelReport>>,
}

static STORE: OnceCell<HistoricalModelStore> = OnceCell::const_new();

impl HistoricalModelStore {
    async fn open() -> anyhow::Result<Self> {
        Ok(Self::default())
    }

    pub async fn global() -> anyhow::Result<&'static Self> {
        STORE.get_or_try_init(Self::open).await
    }

    /// 丢掉一个快照：回执与它那一格几何一起走。共享的 mesh 文件不动——它们是内容寻址的，
    /// 由可达性 GC 收，不跟着某一个快照走。
    pub async fn drop_snapshot(&self, snapshot_key: &str) -> anyhow::Result<()> {
        validate_snapshot_key(snapshot_key)?;
        ModelMemoryStore::global().drop_scope(ProjectionScope::Historical(snapshot_key));
        self.reports
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(snapshot_key);
        Ok(())
    }

    pub async fn snapshot(&self, snapshot_key: &str) -> anyhow::Result<Option<Value>> {
        validate_snapshot_key(snapshot_key)?;
        self.reports
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(snapshot_key)
            .map(serde_json::to_value)
            .transpose()
            .map_err(Into::into)
    }

    pub async fn query(&self, snapshot_key: &str, tool: &str) -> anyhow::Result<Value> {
        validate_snapshot_key(snapshot_key)?;
        if tool == "snapshot" {
            return Ok(self.snapshot(snapshot_key).await?.unwrap_or(Value::Null));
        }
        let records = ModelMemoryStore::global().records(ProjectionScope::Historical(snapshot_key));
        let rows = match tool {
            "instances" => records
                .iter()
                .filter(|record| record.tube.is_none())
                .map(|record| instance_json(record, snapshot_key))
                .collect::<Vec<_>>(),
            "tubes" => records
                .iter()
                .filter(|record| record.tube.is_some())
                .map(|record| tube_json(record, snapshot_key))
                .collect::<Vec<_>>(),
            "geometry" => {
                let mut meshes = records
                    .iter()
                    .map(|record| (record.mesh_id.as_str(), record))
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into_values()
                    .map(mesh_json)
                    .collect::<Vec<_>>();
                meshes.dedup();
                meshes
            }
            other => anyhow::bail!(
                "unknown historical query tool {other:?}; expected snapshot/instances/tubes/geometry"
            ),
        };
        Ok(Value::Array(rows))
    }

    async fn publish(&self, report: &HistoricalModelReport) -> anyhow::Result<()> {
        self.reports
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(report.snapshot_key.clone(), report.clone());
        Ok(())
    }
}

fn refno_key(refno: RefNo) -> String {
    format!("{}_{}", refno.word0, refno.word1)
}

fn transform_json(matrix: glam::DMat4) -> Value {
    let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
    json!({
        "translation": [translation.x as f32, translation.y as f32, translation.z as f32],
        "rotation": [rotation.x as f32, rotation.y as f32, rotation.z as f32, rotation.w as f32],
        "scale": [scale.x as f32, scale.y as f32, scale.z as f32],
    })
}

fn bounds_json(bounds: [[f32; 3]; 2]) -> Value {
    json!({ "mins": bounds[0], "maxs": bounds[1] })
}

fn geometry_format(shape: &ProjectedShape) -> &'static str {
    match shape {
        ProjectedShape::Canonical { .. } => "canonical-primitive-v1",
        ProjectedShape::Baked => "baked-v2",
    }
}

fn direct_model_json(record: &ProjectedGeometry) -> Value {
    json!({
        "source": "e3d-model",
        "format": geometry_format(&record.shape),
        "sesno": record.source_sesno,
        "geometry_id": record.geometry_id,
        "mesh_id": record.mesh_id,
    })
}

fn instance_json(record: &ProjectedGeometry, snapshot_key: &str) -> Value {
    let mut row = json!({
        "id": record.record_id,
        "snapshot_key": snapshot_key,
        "dbnum": record.dbnum,
        "generic": record.noun,
        "solid": true,
        "anc": record.ancestors,
        "source_refno": refno_key(record.source_refno),
        "mesh_id": record.mesh_id,
        "world_bounds": bounds_json(record.world_bounds),
        "world_transform": transform_json(record.world_transform),
        "direct_model": direct_model_json(record),
    });
    match &record.shape {
        ProjectedShape::Canonical {
            key,
            local_transform,
            local_bounds,
        } => {
            row["booled"] = json!(false);
            row["primitive_key"] = json!(key);
            row["local_transform"] = transform_json(*local_transform);
            row["local_bounds"] = bounds_json(*local_bounds);
        }
        ProjectedShape::Baked => {
            row["booled"] = json!(true);
            row["booled_id"] = json!(record.mesh_id);
        }
    }
    row
}

fn tube_json(record: &ProjectedGeometry, snapshot_key: &str) -> Value {
    let tube = record.tube.as_ref().expect("tube record");
    json!({
        "id": record.record_id,
        "snapshot_key": snapshot_key,
        "dbnum": record.dbnum,
        "container_refno": refno_key(record.source_refno),
        "source_refno": refno_key(record.source_refno),
        "leave_refno": refno_key(tube.from),
        "arrive_refno": refno_key(tube.to),
        "route_ordinal": tube.route_ordinal,
        "bore_size": 0,
        "invalid": false,
        "anc": record.ancestors,
        "mesh_id": record.mesh_id,
        "world_bounds": bounds_json(record.world_bounds),
        "world_transform": transform_json(record.world_transform),
        "direct_model": direct_model_json(record),
    })
}

fn mesh_json(record: &ProjectedGeometry) -> Value {
    json!({
        "id": record.mesh_id,
        "mesh": format!("{}.mesh", record.mesh_id),
        "meshed": true,
        "visible": true,
        "bad": false,
        "direct_model": {
            "source": "e3d-model",
            "format": geometry_format(&record.shape),
        },
    })
}

pub async fn generate_historical(
    request: HistoricalModelRequest,
) -> Result<HistoricalModelReport, HistoricalModelError> {
    let started = Instant::now();
    if request.refno.dbno() != request.dbnum {
        return Err(HistoricalModelError::new(
            "INVALID_SELECTOR",
            format!(
                "refno {} belongs to dbnum {}, not requested dbnum {}",
                request.refno,
                request.refno.dbno(),
                request.dbnum
            ),
        ));
    }

    let service = source_service().map_err(|e| HistoricalModelError::new("SOURCE_NOT_FOUND", e))?;
    let source_file = service
        .source_file(request.dbnum)
        .map_err(|e| HistoricalModelError::new("SOURCE_NOT_FOUND", e))?
        .to_path_buf();
    let resolved = resolve_session(&source_file, &request.selector)?;
    let source_fingerprint = source_fingerprint(&source_file)
        .map_err(|e| HistoricalModelError::new("GEOMETRY_GENERATION_FAILED", e))?;
    let snapshot_key = format!(
        "{}_{}@{}",
        request.refno.word0, request.refno.word1, resolved.sesno
    );
    validate_snapshot_key(&snapshot_key)
        .map_err(|e| HistoricalModelError::new("INVALID_SELECTOR", e))?;

    let generated = service
        .generate_snapshot_source(
            request.dbnum,
            request.refno,
            resolved.sesno,
            request.failure_policy,
        )
        .await
        .map_err(classify_generation_error)?;
    if generated.report.visited > MAX_HISTORY_ELEMENTS {
        return Err(HistoricalModelError::new(
            "GENERATION_LIMIT_EXCEEDED",
            format!(
                "visited {} elements; limit is {MAX_HISTORY_ELEMENTS}",
                generated.report.visited
            ),
        ));
    }

    let store = HistoricalModelStore::global()
        .await
        .map_err(|e| HistoricalModelError::new("SNAPSHOT_COMMIT_FAILED", e))?;
    store
        .drop_snapshot(&snapshot_key)
        .await
        .map_err(|e| HistoricalModelError::new("SNAPSHOT_COMMIT_FAILED", e))?;
    let option = aios_core::get_db_option();
    let persisted = apply_geometry_delta_on(
        ProjectionScope::Historical(&snapshot_key),
        request.dbnum,
        resolved.sesno,
        generated.elements,
        Vec::new(),
        &generated.owners,
        &option.get_meshes_path(),
    )
    .await
    .map_err(|e| HistoricalModelError::new("SNAPSHOT_COMMIT_FAILED", e))?;

    let report = HistoricalModelReport {
        snapshot_key,
        dbnum: request.dbnum,
        refno: request.refno.to_string(),
        resolved_session: resolved,
        source_fingerprint,
        source_file: source_file.display().to_string(),
        visited: generated.report.visited,
        generated: generated.report.generated,
        failed: generated.report.failed.len(),
        skipped: generated.report.skipped.len(),
        upserted: persisted.upserted,
        shared_instances: persisted.shared_instances,
        baked_instances: persisted.baked_instances,
        mesh_written: persisted.mesh_written,
        mesh_reused: persisted.mesh_reused,
        unique_meshes: persisted.unique_meshes,
        elapsed_ms: started.elapsed().as_millis(),
    };
    store
        .publish(&report)
        .await
        .map_err(|e| HistoricalModelError::new("SNAPSHOT_COMMIT_FAILED", e))?;
    log::info!(
        "historical model ready snapshot_key={} dbnum={} refno={} sesno={} visited={} generated={} failed={} elapsed_ms={}",
        report.snapshot_key,
        report.dbnum,
        report.refno,
        report.resolved_session.sesno,
        report.visited,
        report.generated,
        report.failed,
        report.elapsed_ms
    );
    Ok(report)
}

/// 把 `generate_snapshot_source` 的失败分成两个错误码。**按类型分，不按消息文本分。**
///
/// 此前靠 `message.contains("索引") || message.contains("not found")`，而 `message` 又是
/// `e.to_string()`——anyhow 只给最外层那句 `generate root …`，底下的原因整条被吃掉，
/// 于是「根在那一会话根本不存在」永远落成 `GEOMETRY_GENERATION_FAILED`，消息里还没有原因
/// （2026-09-13 live 回执 `verification/historical-model-live-20260913-2233` 用例 3）。
/// 现在源侧用 [`SnapshotSourceError`] 说话，这里 downcast；其余一律是几何失败，消息用
/// `{:#}` 带全整条因果链。
fn classify_generation_error(error: anyhow::Error) -> HistoricalModelError {
    let code = match error.downcast_ref::<SnapshotSourceError>() {
        Some(SnapshotSourceError::RootNotInSession { .. }) => "REFNO_NOT_FOUND_AT_SESSION",
        None => "GEOMETRY_GENERATION_FAILED",
    };
    // `{:#}` 才带整条因果链；`to_string()` 只给最外层那句 context。
    HistoricalModelError::new(code, format!("{error:#}"))
}

pub(crate) fn source_service() -> anyhow::Result<E3dModelService> {
    let option = aios_core::get_db_option();
    let (pins, locator) = current_mdb_sources()?;
    E3dModelService::from_source_files(pins, locator, option.get_meshes_path())
}

/// 解一个库文件上的时点选择。**当前投影解「最新」也走这里**（ADR-054 实施约束 1：一把尺子），
/// 见 `data_interface::model_source`。
pub(crate) fn resolve_session(
    path: &Path,
    selector: &SessionSelector,
) -> Result<ResolvedSession, HistoricalModelError> {
    match selector {
        SessionSelector::Latest => {
            let engine = ReadOnlyEngine::open(path)
                .map_err(|e| HistoricalModelError::new("SESSION_NOT_FOUND", e))?;
            let sesno = engine.session().sesno;
            Ok(ResolvedSession {
                requested: "latest".to_string(),
                sesno,
                session_time: session_chain_times(path)
                    .ok()
                    .and_then(|chain| time_of(&chain, sesno)),
            })
        }
        SessionSelector::Sesno(sesno) => {
            ReadOnlyEngine::open_at(path, *sesno)
                .map_err(|e| HistoricalModelError::new("SESSION_NOT_FOUND", e))?;
            Ok(ResolvedSession {
                requested: format!("sesno:{sesno}"),
                sesno: *sesno,
                session_time: session_chain_times(path)
                    .ok()
                    .and_then(|chain| time_of(&chain, *sesno)),
            })
        }
        SessionSelector::At(requested) => {
            let chain = session_chain_times(path)?;
            let (sesno, time) = select_session_at(&chain, requested)?;
            Ok(ResolvedSession {
                requested: format!("time:{}", requested.to_rfc3339()),
                sesno,
                session_time: Some(time),
            })
        }
    }
}

/// 时点 `t` 落在哪条会话上：**从链头往回走，第一条时刻 ≤ t 的会话**。
///
/// 按时刻取最大值是错的。会话 1 的时刻是这个文件**在本项目里被建立**的时刻，而
/// 会话 2 以后带的是从源项目拷进来的**原始创作时刻**，所以会话 1 常常比它后面
/// 整条链都晚：AvevaMarineSample 的 443 个库里 423 个如此，最大倒退 13.6 年。
/// 纯时刻序会让这些库的**任何** `time` 请求都解到会话 1——初始树——而且不报错、
/// 回执里还显示一个看起来正常的日期。实测 36 个库当场中招（含一个目录库），
/// 取证见 `docs/evidence/2026-09-08-session-time-coverage-and-selector-probe.md` §3。
///
/// 链序才是「谁在谁之前」的权威。这与 `e3d_io::session::SessionSelector::AtOrBefore`
/// 对 sesno 的解法、以及 `e3d_model::increment::collect_window` 判窗口方向时
/// 「链序为准——链不保证连号，数字大小不算数」是同一把尺子。
fn select_session_at(
    chain: &[ChainTime],
    requested: &DateTime<FixedOffset>,
) -> Result<(u32, String), HistoricalModelError> {
    if chain.is_empty() {
        return Err(HistoricalModelError::new(
            "SESSION_TIME_UNAVAILABLE",
            "source has no decodable session timestamps",
        ));
    }
    let requested_utc = requested.with_timezone(&Utc);
    for entry in chain.iter().rev() {
        let time = DateTime::parse_from_rfc3339(&entry.time)
            .map_err(|e| HistoricalModelError::new("SESSION_TIME_UNAVAILABLE", e))?;
        if time.with_timezone(&Utc) <= requested_utc {
            return Ok((entry.sesno, entry.time.clone()));
        }
    }
    Err(HistoricalModelError::new(
        "TIME_BEFORE_OLDEST_SESSION",
        requested.to_rfc3339(),
    ))
}

/// 一条会话的号与时刻。**装在 `Vec` 里而不是 `HashMap`，因为顺序是链序**——
/// 谁在谁之前只有链说了算，见 [`select_session_at`]。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChainTime {
    sesno: u32,
    time: String,
}

/// 整条会话链的时刻，**旧 → 新**（`session_chain` 就是这个方向）。
///
/// 整条链一次读完（e3d-io `DbView::walk_sessions`），时刻解法与旧 `get_dt` 相同；
/// 解不出时刻的会话页（字段全零）不进候选，与旧实现按 `get_dt` 必成功的假设相比更保守。
fn session_chain_times(path: &Path) -> Result<Vec<ChainTime>, HistoricalModelError> {
    let chain = crate::data_interface::db_file_header::session_chain(path)
        .map_err(|e| HistoricalModelError::new("SESSION_TIME_UNAVAILABLE", e))?;
    Ok(chain
        .iter()
        .filter_map(|stamp| {
            stamp.rfc3339().map(|time| ChainTime {
                sesno: stamp.sesno,
                time,
            })
        })
        .collect())
}

fn time_of(chain: &[ChainTime], sesno: u32) -> Option<String> {
    chain
        .iter()
        .find(|entry| entry.sesno == sesno)
        .map(|entry| entry.time.clone())
}

pub(crate) fn source_fingerprint(path: &Path) -> anyhow::Result<String> {
    let meta = std::fs::metadata(path)?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    let mut hash = Sha256::new();
    hash.update(path.as_os_str().to_string_lossy().as_bytes());
    hash.update(meta.len().to_be_bytes());
    hash.update(modified.to_be_bytes());
    Ok(hex::encode(hash.finalize()))
}

fn validate_snapshot_key(value: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'_' | b'@')),
        "invalid snapshot key {value:?}"
    );
    Ok(())
}

pub fn parse_selector(
    sesno: Option<u32>,
    time: Option<&str>,
) -> Result<SessionSelector, HistoricalModelError> {
    match (sesno, time) {
        (Some(_), Some(_)) => Err(HistoricalModelError::new(
            "INVALID_SELECTOR",
            "sesno and time are mutually exclusive",
        )),
        (Some(value), None) => Ok(SessionSelector::Sesno(value)),
        (None, Some(value)) => DateTime::parse_from_rfc3339(value)
            .map(SessionSelector::At)
            .map_err(|e| HistoricalModelError::new("INVALID_SELECTOR", e)),
        (None, None) => Ok(SessionSelector::Latest),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 链序：旧 → 新，与 `db_file_header::session_chain` 同向。
    fn chain_of(entries: &[(u32, &str)]) -> Vec<ChainTime> {
        entries
            .iter()
            .map(|(sesno, time)| ChainTime {
                sesno: *sesno,
                time: (*time).to_string(),
            })
            .collect()
    }

    #[test]
    fn selector_requires_one_or_zero_inputs() {
        assert!(matches!(
            parse_selector(None, None).unwrap(),
            SessionSelector::Latest
        ));
        assert!(matches!(
            parse_selector(Some(24), None).unwrap(),
            SessionSelector::Sesno(24)
        ));
        assert_eq!(
            parse_selector(Some(24), Some("2026-01-01T00:00:00Z"))
                .unwrap_err()
                .code,
            "INVALID_SELECTOR"
        );
        assert_eq!(
            parse_selector(None, Some("2026-01-01 00:00:00"))
                .unwrap_err()
                .code,
            "INVALID_SELECTOR"
        );
    }

    /// 根在那一会话不存在 = `REFNO_NOT_FOUND_AT_SESSION`，判据是类型，不是消息里有没有某个词；
    /// 外面再包几层 context 也认得出来。
    #[test]
    fn a_root_missing_from_the_session_index_is_not_a_geometry_failure() {
        let error = anyhow::Error::from(SnapshotSourceError::RootNotInSession {
            root: RefNo::new(24384, 26229),
            dbnum: 8000,
            sesno: 100,
        })
        .context("generate root 24384/26229");
        let classified = classify_generation_error(error);
        assert_eq!(classified.code, "REFNO_NOT_FOUND_AT_SESSION");
        assert!(
            classified.message.contains("24384/26229")
                && classified.message.contains("8000")
                && classified.message.contains("100"),
            "消息要点名根、库、会话：{}",
            classified.message
        );
    }

    /// 其余失败是几何失败，而且消息要带全因果链——只给最外层那句 `generate root …`
    /// 等于没说原因（改回 `e.to_string()` 这一条立刻变红）。
    #[test]
    fn other_generation_failures_keep_the_whole_cause_chain() {
        let error = anyhow::anyhow!("catalogue SPRE /X missing")
            .context("evaluate element 24384/26230")
            .context("generate root 24384/26229");
        let classified = classify_generation_error(error);
        assert_eq!(classified.code, "GEOMETRY_GENERATION_FAILED");
        assert!(
            classified.message.contains("generate root 24384/26229")
                && classified.message.contains("catalogue SPRE /X missing"),
            "{}",
            classified.message
        );
        // 字面上撞到旧的关键词也不再改判：分类不看文本。
        let text_only = anyhow::anyhow!("element not found in 索引");
        assert_eq!(
            classify_generation_error(text_only).code,
            "GEOMETRY_GENERATION_FAILED"
        );
    }

    #[test]
    fn snapshot_keys_are_refno_plus_session() {
        assert!(validate_snapshot_key("24384_24775@24").is_ok());
        assert!(validate_snapshot_key("24384_24775@24';DELETE").is_err());
    }

    #[test]
    fn time_selector_compares_in_utc_and_walks_back_from_the_chain_head() {
        // 24 与 25 是同一个 UTC 时刻，只是写法不同。
        let chain = chain_of(&[
            (24, "2026-01-01T08:00:00+08:00"),
            (25, "2026-01-01T00:00:00Z"),
            (26, "2026-01-02T00:00:00Z"),
        ]);
        let between = DateTime::parse_from_rfc3339("2026-01-01T12:00:00+08:00").unwrap();
        assert_eq!(select_session_at(&chain, &between).unwrap().0, 25);
        let late = DateTime::parse_from_rfc3339("2027-01-01T00:00:00Z").unwrap();
        assert_eq!(select_session_at(&chain, &late).unwrap().0, 26);
        let early = DateTime::parse_from_rfc3339("2025-12-31T23:59:59Z").unwrap();
        assert_eq!(
            select_session_at(&chain, &early).unwrap_err().code,
            "TIME_BEFORE_OLDEST_SESSION"
        );
    }

    /// **改回按时刻取最大值，这一条立刻变红。**
    ///
    /// 形状照抄目录库 `ams251049_0001`：会话 1 停在 2016-01-05（文件在本项目里被
    /// 建立的时刻），会话 2..64 是 2014–2015 的原始创作时刻，比会话 1 都早。
    /// 纯时刻序会让「给我此刻的模型」解到会话 1——初始树——而且不报错。
    #[test]
    fn a_session_one_stamped_after_the_whole_chain_never_wins() {
        let chain = chain_of(&[
            (1, "2016-01-05T09:12:00+08:00"),
            (2, "2014-08-06T10:00:00+08:00"),
            (37, "2015-03-11T14:20:00+08:00"),
            (64, "2015-08-19T16:45:00+08:00"),
        ]);

        let now = DateTime::parse_from_rfc3339("2026-09-08T19:00:00+08:00").unwrap();
        assert_eq!(
            select_session_at(&chain, &now).unwrap().0,
            64,
            "此刻的模型是链头，不是时刻最晚的那条"
        );

        // 越过会话 1 的时刻之后，答案仍然是链头：它在链上排在最前面。
        let after_session_one = DateTime::parse_from_rfc3339("2016-06-01T00:00:00+08:00").unwrap();
        assert_eq!(select_session_at(&chain, &after_session_one).unwrap().0, 64);

        // 真正的历史时点照常沿链回走。
        let mid = DateTime::parse_from_rfc3339("2015-06-01T00:00:00+08:00").unwrap();
        assert_eq!(select_session_at(&chain, &mid).unwrap().0, 37);

        // 早于整条链 = 报错，不拿会话 1 顶替。
        let before = DateTime::parse_from_rfc3339("2014-01-01T00:00:00+08:00").unwrap();
        assert_eq!(
            select_session_at(&chain, &before).unwrap_err().code,
            "TIME_BEFORE_OLDEST_SESSION"
        );
    }

    /// 全语料门：**「此刻的模型」必须是每个库的链头，一个库都不许例外。**
    ///
    /// 这一条守的是上面那个合成用例的真实来源。2026-09-08 在
    /// `AvevaMarineSample\ams000` 上实测：439 个库里 36 个（PADD 17、XPRD 7、
    /// SCHE 5、ISOD 3、DICT/PROP/ALYS/CATA 各 1）在按时刻取最大值的旧解法下
    /// 会解到会话 1。取证见
    /// `docs/evidence/2026-09-08-session-time-coverage-and-selector-probe.md` §3。
    #[test]
    #[ignore = "manual live: needs the AvevaMarineSample ams000 project directory"]
    fn live_ams000_now_resolves_to_every_chain_head() {
        let project = PathBuf::from(
            std::env::var("AIOS_AMS_PROJECT_DIR")
                .unwrap_or_else(|_| r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000".into()),
        );
        let now: DateTime<FixedOffset> = Utc::now().into();

        let mut checked = 0usize;
        let mut wrong: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&project).expect("project directory") {
            let path = entry.expect("dir entry").path();
            if !path.is_file() {
                continue;
            }
            let Ok(engine) = ReadOnlyEngine::open(&path) else {
                continue; // 不是 dabacon 文件
            };
            let head = engine.session().sesno;
            let Ok(chain) = session_chain_times(&path) else {
                continue;
            };
            if chain.len() < 2 {
                continue;
            }
            checked += 1;
            let picked = select_session_at(&chain, &now)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
                .0;
            if picked != head {
                wrong.push(format!(
                    "{}: 解到会话 {picked}，链头是 {head}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
        }

        // 手工门，跑的人要看得见覆盖了多少个库，否则「全绿」可能只是没读到文件。
        println!("checked {checked} databases under {}", project.display());
        assert!(checked > 0, "{} 下没读到任何库文件", project.display());
        assert!(
            wrong.is_empty(),
            "{} / {checked} 个库把「此刻」解到了链头以外：\n{}",
            wrong.len(),
            wrong.join("\n")
        );
    }

    #[test]
    fn a_session_time_is_looked_up_by_number_not_by_position() {
        let chain = chain_of(&[
            (1, "2016-01-05T09:12:00+08:00"),
            (64, "2015-08-19T16:45:00+08:00"),
        ]);
        assert_eq!(
            time_of(&chain, 64).as_deref(),
            Some("2015-08-19T16:45:00+08:00")
        );
        assert_eq!(time_of(&chain, 63), None);
    }

    /// 丢快照要把它那一格几何与回执一起清掉，别的快照不受影响；共享 mesh 是内容寻址的
    /// 盘上文件，不跟着某一个快照走。
    #[tokio::test]
    async fn dropping_snapshot_removes_its_namespace_but_leaves_other_snapshots() {
        use crate::fast_model::model_memory_store::{
            ProjectedGeometry, ProjectedShape, ProjectionCommit,
        };

        let store = HistoricalModelStore::open().await.expect("report store");
        let key = "24384_24775@24";
        let neighbour = "24384_24775@25";
        let report = HistoricalModelReport {
            snapshot_key: key.to_string(),
            dbnum: 24384,
            refno: "24384/24775".to_string(),
            resolved_session: ResolvedSession {
                requested: "sesno:24".to_string(),
                sesno: 24,
                session_time: None,
            },
            source_file: "fixture".to_string(),
            source_fingerprint: "fingerprint".to_string(),
            visited: 1,
            generated: 1,
            failed: 0,
            skipped: 0,
            upserted: 1,
            shared_instances: 1,
            baked_instances: 0,
            mesh_written: 0,
            mesh_reused: 1,
            unique_meshes: 1,
            elapsed_ms: 1,
        };
        store.publish(&report).await.expect("publish snapshot");

        let projection = ModelMemoryStore::global();
        let seed = |record_id: &str| ProjectionCommit {
            upserts: vec![ProjectedGeometry {
                record_id: record_id.to_string(),
                geometry_id: e3d_model::elmodl::GeometryId::Element {
                    refno: "24384/24775".into(),
                },
                source_refno: RefNo::new(24384, 24775),
                dbnum: 24384,
                source_sesno: 24,
                noun: "EQUI".into(),
                mesh_id: "shared_mesh_hash".into(),
                world_transform: glam::DMat4::IDENTITY,
                world_bounds: [[0.0; 3], [1.0; 3]],
                ancestors: vec![104_722_154_390_487],
                shape: ProjectedShape::Baked,
                tube: None,
            }],
            ..ProjectionCommit::default()
        };
        projection.commit(ProjectionScope::Historical(key), vec![seed("24384_24775")]);
        projection.commit(
            ProjectionScope::Historical(neighbour),
            vec![seed("24384_24775")],
        );

        assert!(store.snapshot(key).await.expect("read snapshot").is_some());
        assert_eq!(
            store.query(key, "instances").await.unwrap()[0]["id"],
            "24384_24775"
        );
        assert_eq!(
            store.query(key, "geometry").await.unwrap()[0]["mesh"],
            "shared_mesh_hash.mesh"
        );

        store.drop_snapshot(key).await.expect("drop snapshot");
        assert!(store.snapshot(key).await.expect("read dropped").is_none());
        assert_eq!(store.query(key, "instances").await.unwrap(), json!([]));
        assert_eq!(
            store.query(neighbour, "instances").await.unwrap()[0]["id"],
            "24384_24775",
            "别的快照不该被这一次丢连累"
        );
        projection.drop_scope(ProjectionScope::Historical(neighbour));
    }
}
