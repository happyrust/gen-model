//! On-demand historical model projection.
//!
//! Source reads are session-pinned e3d-io reads.  The resulting model rows live
//! in a dedicated `mem://` SurrealDB and never touch the production `SUL_DB`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Instant, UNIX_EPOCH};

use anyhow::Context;
use chrono::{DateTime, FixedOffset, Utc};
use e3d_io::ReadOnlyEngine;
use e3d_io::refno::RefNo;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use surrealdb::Surreal;
use surrealdb::engine::any::{Any, connect};
use tokio::sync::OnceCell;

use crate::data_interface::geom_error::GeometryFailurePolicy;
use crate::fast_model::e3d_model_service::{
    E3dModelService, ProjectionScope, apply_geometry_delta_on, current_mdb_sources,
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

pub struct HistoricalModelStore {
    db: Surreal<Any>,
}

static STORE: OnceCell<HistoricalModelStore> = OnceCell::const_new();

impl HistoricalModelStore {
    async fn open() -> anyhow::Result<Self> {
        let db = connect("mem://").await?;
        db.use_ns("aios_history").use_db("model_snapshots").await?;
        Ok(Self { db })
    }

    pub async fn global() -> anyhow::Result<&'static Self> {
        STORE.get_or_try_init(Self::open).await
    }

    pub fn db(&self) -> &Surreal<Any> {
        &self.db
    }

    pub async fn drop_snapshot(&self, snapshot_key: &str) -> anyhow::Result<()> {
        validate_snapshot_key(snapshot_key)?;
        let sql = format!(
            "BEGIN TRANSACTION;\n\
             DELETE geo_relate WHERE snapshot_key='{snapshot_key}';\n\
             DELETE tubi_relate WHERE snapshot_key='{snapshot_key}';\n\
             DELETE inst_relate WHERE snapshot_key='{snapshot_key}';\n\
             DELETE inst_info WHERE snapshot_key='{snapshot_key}';\n\
             DELETE trans WHERE snapshot_key='{snapshot_key}';\n\
             DELETE aabb WHERE snapshot_key='{snapshot_key}';\n\
             DELETE type::thing('historical_snapshot','{snapshot_key}');\n\
             COMMIT TRANSACTION;"
        );
        self.db.query(sql).await?.check()?;
        Ok(())
    }

    pub async fn snapshot(&self, snapshot_key: &str) -> anyhow::Result<Option<Value>> {
        validate_snapshot_key(snapshot_key)?;
        let mut response = self
            .db
            .query(format!(
                "SELECT * FROM type::thing('historical_snapshot','{snapshot_key}');"
            ))
            .await?
            .check()?;
        Ok(response
            .take::<Vec<HistoricalModelReport>>(0)?
            .into_iter()
            .next()
            .map(serde_json::to_value)
            .transpose()?)
    }

    pub async fn query(&self, snapshot_key: &str, tool: &str) -> anyhow::Result<Value> {
        validate_snapshot_key(snapshot_key)?;
        let sql = match tool {
            "instances" => format!(
                "SELECT record::id(id) AS id, snapshot_key, dbnum, generic, booled_id, booled, \
                 bad_bool, solid, anc, insts_flat, direct_model, record::id(in) AS source_refno, \
                 IF type::is::record(out) THEN record::id(out) ELSE NONE END AS inst_info_id, \
                 IF type::is::record(aabb) THEN record::id(aabb) ELSE NONE END AS aabb_id, \
                 IF type::is::record(world_trans) THEN record::id(world_trans) ELSE NONE END AS world_trans_id \
                 FROM inst_relate WHERE snapshot_key='{snapshot_key}';"
            ),
            "tubes" => format!(
                "SELECT record::id(id) AS id, snapshot_key, source_refno, dbnum, bore_size, invalid, \
                 anc, direct_model, record::id(in) AS container_refno, record::id(out) AS mesh_id, \
                 record::id(leave) AS leave_refno, record::id(arrive) AS arrive_refno, \
                 record::id(aabb) AS aabb_id, record::id(world_trans) AS world_trans_id \
                 FROM tubi_relate WHERE snapshot_key='{snapshot_key}';"
            ),
            "geometry" => format!(
                "SELECT record::id(id) AS id, mesh, param, meshed, visible, bad, direct_model, \
                 IF type::is::record(aabb) THEN record::id(aabb) ELSE NONE END AS aabb_id FROM inst_geo \
                 WHERE id IN (SELECT VALUE out FROM geo_relate WHERE snapshot_key='{snapshot_key}') \
                 OR id IN (SELECT VALUE out FROM tubi_relate WHERE snapshot_key='{snapshot_key}');"
            ),
            "snapshot" => return Ok(self.snapshot(snapshot_key).await?.unwrap_or(Value::Null)),
            other => anyhow::bail!(
                "unknown historical query tool {other:?}; expected snapshot/instances/tubes/geometry"
            ),
        };
        let mut response = self.db.query(sql).await?.check()?;
        Ok(Value::Array(response.take::<Vec<Value>>(0)?))
    }

    async fn publish(&self, report: &HistoricalModelReport) -> anyhow::Result<()> {
        let value = serde_json::to_string(report)?;
        self.db
            .query(format!(
                "UPSERT type::thing('historical_snapshot','{}') CONTENT {value};",
                report.snapshot_key
            ))
            .await?
            .check()?;
        Ok(())
    }
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
        .map_err(|e| {
            let message = e.to_string();
            let code = if message.contains("索引") || message.contains("not found") {
                "REFNO_NOT_FOUND_AT_SESSION"
            } else {
                "GEOMETRY_GENERATION_FAILED"
            };
            HistoricalModelError::new(code, message)
        })?;
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
        store.db(),
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

fn source_service() -> anyhow::Result<E3dModelService> {
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
                session_time: session_times(path)
                    .ok()
                    .and_then(|mut times| times.remove(&sesno)),
            })
        }
        SessionSelector::Sesno(sesno) => {
            ReadOnlyEngine::open_at(path, *sesno)
                .map_err(|e| HistoricalModelError::new("SESSION_NOT_FOUND", e))?;
            Ok(ResolvedSession {
                requested: format!("sesno:{sesno}"),
                sesno: *sesno,
                session_time: session_times(path)
                    .ok()
                    .and_then(|mut times| times.remove(sesno)),
            })
        }
        SessionSelector::At(requested) => {
            let times = session_times(path)?;
            let (sesno, time) = select_session_at(&times, requested)?;
            Ok(ResolvedSession {
                requested: format!("time:{}", requested.to_rfc3339()),
                sesno,
                session_time: Some(time),
            })
        }
    }
}

fn select_session_at(
    times: &HashMap<u32, String>,
    requested: &DateTime<FixedOffset>,
) -> Result<(u32, String), HistoricalModelError> {
    if times.is_empty() {
        return Err(HistoricalModelError::new(
            "SESSION_TIME_UNAVAILABLE",
            "source has no decodable session timestamps",
        ));
    }
    let requested_utc = requested.with_timezone(&Utc);
    let candidates = times
        .iter()
        .map(|(&sesno, value)| {
            DateTime::parse_from_rfc3339(value)
                .map(|time| (sesno, time.with_timezone(&Utc), value.clone()))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| HistoricalModelError::new("SESSION_TIME_UNAVAILABLE", e))?;
    candidates
        .into_iter()
        .filter(|(_, time, _)| *time <= requested_utc)
        .max_by_key(|(sesno, time, _)| (*time, *sesno))
        .map(|(sesno, _, time)| (sesno, time))
        .ok_or_else(|| {
            HistoricalModelError::new("TIME_BEFORE_OLDEST_SESSION", requested.to_rfc3339())
        })
}

fn session_times(path: &Path) -> Result<HashMap<u32, String>, HistoricalModelError> {
    let mut io = pdms_io::io::PdmsIO::new("", path.to_path_buf(), true);
    io.open()
        .map_err(|e| HistoricalModelError::new("SESSION_TIME_UNAVAILABLE", e))?;
    Ok(io
        .ses_data_map
        .values()
        .filter_map(|session| {
            u32::try_from(session.sesno)
                .ok()
                .map(|sesno| (sesno, session.get_dt().to_rfc3339()))
        })
        .collect())
}

fn source_fingerprint(path: &Path) -> anyhow::Result<String> {
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

    #[test]
    fn snapshot_keys_are_refno_plus_session() {
        assert!(validate_snapshot_key("24384_24775@24").is_ok());
        assert!(validate_snapshot_key("24384_24775@24';DELETE").is_err());
    }

    #[test]
    fn time_selector_uses_utc_then_highest_session_number() {
        let times = HashMap::from([
            (24, "2026-01-01T08:00:00+08:00".to_string()),
            (25, "2026-01-01T00:00:00Z".to_string()),
            (26, "2026-01-02T00:00:00Z".to_string()),
        ]);
        let between = DateTime::parse_from_rfc3339("2026-01-01T12:00:00+08:00").unwrap();
        assert_eq!(select_session_at(&times, &between).unwrap().0, 25);
        let late = DateTime::parse_from_rfc3339("2027-01-01T00:00:00Z").unwrap();
        assert_eq!(select_session_at(&times, &late).unwrap().0, 26);
        let early = DateTime::parse_from_rfc3339("2025-12-31T23:59:59Z").unwrap();
        assert_eq!(
            select_session_at(&times, &early).unwrap_err().code,
            "TIME_BEFORE_OLDEST_SESSION"
        );
    }

    #[tokio::test]
    async fn dropping_snapshot_removes_namespaced_rows_but_keeps_shared_mesh_rows() {
        let store = HistoricalModelStore::open().await.expect("mem store");
        let key = "24384_24775@24";
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
        store
            .db()
            .query(format!(
                "CREATE pe:24384_24775;\
                 CREATE inst_relate:⟨{key}::24384_24775⟩ SET snapshot_key='{key}',in=pe:24384_24775;\
                 CREATE inst_geo:⟨shared_mesh_hash⟩ SET mesh='shared_mesh_hash.mesh';"
            ))
            .await
            .expect("seed rows")
            .check()
            .expect("seed statements");

        assert!(store.snapshot(key).await.expect("read snapshot").is_some());
        assert_eq!(
            store.query(key, "instances").await.unwrap()[0]["id"],
            "24384_24775@24::24384_24775"
        );
        store.drop_snapshot(key).await.expect("drop snapshot");
        assert!(store.snapshot(key).await.expect("read dropped").is_none());
        assert_eq!(store.query(key, "instances").await.unwrap(), json!([]));
        let mut response = store
            .db()
            .query("RETURN count(SELECT VALUE id FROM inst_geo:⟨shared_mesh_hash⟩);")
            .await
            .expect("read shared mesh")
            .check()
            .expect("read statement");
        assert_eq!(response.take::<Option<u64>>(0).unwrap(), Some(1));
    }
}
