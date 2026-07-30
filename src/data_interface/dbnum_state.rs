//! DbnumState — authoritative per-`dbnum` incremental state (see ADR-001).
//!
//! One row per `dbnum`, physically the existing `dbnum_watermark:{dbnum}` record,
//! extended with file identity + scan-observation fields. `applied_sesno` is the
//! ONLY authoritative watermark (advanced after a data batch fully persists);
//! `file_latest_sesno` is a scan observation and must never substitute for it.
//!
//! Read semantics (ADR-001 §兼容迁移):
//! 1. Prefer an already-established `applied_sesno`.
//! 2. Otherwise inherit the legacy `dbnum_watermark.sesno`.
//! 3. Otherwise (only when no dedicated watermark exists) fall back once to the
//!    max `sesno` in `dbnum_info_table` for this `dbnum`.
//!
//! After the state is established (a scan / advance writes `applied_sesno`), reads
//! use `applied_sesno` directly and never re-mix other sources.

use aios_core::SUL_DB;
use serde::{Deserialize, Serialize};

/// Authoritative per-`dbnum` state table (extends the legacy watermark record).
pub const WATERMARK_TABLE: &str = "dbnum_watermark";
/// Legacy per-`ref_0` element-statistics table, used only for one-time migration.
pub const INFO_TABLE: &str = "dbnum_info_table";

/// File observation captured during a (read-only) scan.
///
/// Writing this must NOT touch `applied_sesno` beyond a one-time establishment
/// migration; it only refreshes the scan-observation fields and `scanned_at`.
#[derive(Debug, Clone, Default)]
pub struct FileObservation {
    pub dbnum: u32,
    pub db_type: String,
    pub file_name: String,
    pub file_path: String,
    pub file_size: u64,
    pub file_latest_sesno: i32,
    /// RFC3339 timestamp of the file's last-modified time, if known.
    pub file_modified_at: Option<String>,
}

/// Effective DBNUM state resolved from the stored record (+ one-time migration).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DbnumState {
    pub dbnum: u32,
    pub db_type: String,
    pub file_name: String,
    pub file_path: String,
    pub file_size: u64,
    pub file_latest_sesno: i32,
    /// Effective applied watermark (migrated when necessary); 0 when uninitialized.
    pub applied_sesno: i32,
    /// `true` when a watermark could be resolved from any source (record, legacy
    /// field or info table). `false` means this `dbnum` has never been applied.
    pub initialized: bool,
}

/// Raw projection of the stored `dbnum_watermark:{dbnum}` record used for reads.
///
/// Only non-datetime fields are selected so deserialization stays trivial.
#[derive(Debug, Clone, Default, Deserialize)]
struct StateRow {
    #[serde(default)]
    dbnum: Option<u32>,
    #[serde(default)]
    db_type: Option<String>,
    #[serde(default)]
    file_name: Option<String>,
    #[serde(default)]
    file_path: Option<String>,
    #[serde(default)]
    file_size: Option<u64>,
    #[serde(default)]
    file_latest_sesno: Option<i32>,
    /// New authoritative field; `None` when not yet established (pre-migration).
    #[serde(default)]
    applied_sesno: Option<i32>,
    /// Legacy watermark field, kept for migration + backward-compat mirroring.
    #[serde(default)]
    sesno: Option<i32>,
}

/// A file-identity anomaly for one `dbnum` (see spec §文件异常).
///
/// [`check_file_against_state`] decides `Rollback` / `PathMigrated` from a single
/// observed file vs the stored state; `Duplicate` / `Missing` are constructed by
/// the project scanner which aggregates all files per `dbnum`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileAnomaly {
    /// `file_latest_sesno < applied_sesno`: the file rolled back or was replaced.
    /// The `dbnum` must be blocked; the watermark must NOT regress.
    Rollback {
        file_latest_sesno: i32,
        applied_sesno: i32,
    },
    /// Same `dbnum` and `db_type`, path changed, watermark did not regress:
    /// a unique file was moved and the stored path may be auto-updated.
    PathMigrated { old_path: String, new_path: String },
    /// Same dbnum was observed with a different database type. Never overwrite
    /// the stored identity automatically.
    TypeChanged {
        stored_db_type: String,
        observed_db_type: String,
    },
    /// Multiple files with the same `dbnum` in the project: block, do not pick.
    Duplicate { paths: Vec<String> },
    /// A registered file is no longer present at its recorded path.
    Missing { path: String },
}

impl FileAnomaly {
    /// 该异常是否阻断执行（不入队、不应用）。
    ///
    /// 五种异常里**只有路径迁移不阻断**——它是良性搬家，登记路径跟着更新即可
    /// （QUEUE-FIELD-MAP §3「本期不执行」一格的判定，从预览里提出来复用）。
    pub fn blocks(&self) -> bool {
        !matches!(self, FileAnomaly::PathMigrated { .. })
    }
}

/// Resolve the effective applied watermark from the three ordered sources.
///
/// Pure decision function (ADR-001 §兼容迁移). Priority:
/// established `applied_sesno` > legacy `dbnum_watermark.sesno` > `dbnum_info_table`
/// max. Returns `None` when nothing is known (uninitialized `dbnum`).
pub fn resolve_migrated_applied_sesno(
    existing_applied: Option<i32>,
    legacy_watermark_sesno: Option<i32>,
    info_table_max_sesno: Option<i32>,
) -> Option<i32> {
    existing_applied
        .or(legacy_watermark_sesno)
        .or(info_table_max_sesno)
}

/// Classify one observed file for one `dbnum` against its stored state.
///
/// Returns `Some(anomaly)` when there is something to report/handle, `None` when
/// the file looks normal. Rollback takes precedence over a path change.
pub fn check_file_against_state(
    stored_db_type: Option<&str>,
    stored_path: Option<&str>,
    applied_sesno: i32,
    observed_db_type: &str,
    observed_path: &str,
    observed_file_latest_sesno: i32,
) -> Option<FileAnomaly> {
    if observed_file_latest_sesno < applied_sesno {
        return Some(FileAnomaly::Rollback {
            file_latest_sesno: observed_file_latest_sesno,
            applied_sesno,
        });
    }
    if let (Some(stored_path), Some(stored_db_type)) = (stored_path, stored_db_type) {
        if stored_db_type != observed_db_type {
            return Some(FileAnomaly::TypeChanged {
                stored_db_type: stored_db_type.to_string(),
                observed_db_type: observed_db_type.to_string(),
            });
        }
        if stored_db_type == observed_db_type && stored_path != observed_path {
            return Some(FileAnomaly::PathMigrated {
                old_path: stored_path.to_string(),
                new_path: observed_path.to_string(),
            });
        }
    }
    None
}

/// Escape a string for safe embedding inside a single-quoted SurrealQL literal.
///
/// Windows paths carry backslashes, which are escape characters in SurrealQL
/// strings; escape those and single quotes.
pub(crate) fn escape_surql_str(raw: &str) -> String {
    raw.replace('\\', "\\\\").replace('\'', "\\'")
}

impl DbnumState {
    /// List registered DB files. Used by project scans to surface files that
    /// disappeared instead of silently omitting their dbnum.
    pub async fn list_registered() -> anyhow::Result<Vec<DbnumState>> {
        let sql = format!(
            "SELECT dbnum, db_type, file_name, file_path, file_size, file_latest_sesno, \
             applied_sesno, sesno FROM {WATERMARK_TABLE};"
        );
        let mut response = SUL_DB
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("读取 DBNUM 注册表失败: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("读取 DBNUM 注册表语句失败: {e}"))?;
        let rows: Vec<StateRow> = response
            .take(0)
            .map_err(|e| anyhow::anyhow!("解码 DBNUM 注册表失败: {e}"))?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let dbnum = row.dbnum?;
                let effective = resolve_migrated_applied_sesno(row.applied_sesno, row.sesno, None);
                Some(DbnumState {
                    dbnum,
                    db_type: row.db_type.unwrap_or_default(),
                    file_name: row.file_name.unwrap_or_default(),
                    file_path: row.file_path.unwrap_or_default(),
                    file_size: row.file_size.unwrap_or_default(),
                    file_latest_sesno: row.file_latest_sesno.unwrap_or_default(),
                    applied_sesno: effective.unwrap_or_default(),
                    initialized: effective.is_some(),
                })
            })
            .collect())
    }

    /// Read the raw stored row + info-table fallback for one `dbnum`.
    async fn read_row(dbnum: u32) -> anyhow::Result<(Option<StateRow>, Option<i32>)> {
        let sql = format!(
            "SELECT dbnum, db_type, file_name, file_path, file_size, file_latest_sesno, \
             applied_sesno, sesno FROM {WATERMARK_TABLE}:{dbnum};\
             RETURN math::max((SELECT VALUE sesno FROM {INFO_TABLE} WHERE dbnum = {dbnum}));"
        );
        let mut response = SUL_DB
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("读取 DBNUM 状态失败 dbnum={dbnum}: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("读取 DBNUM 状态语句失败 dbnum={dbnum}: {e}"))?;
        let rows: Vec<StateRow> = response
            .take(0)
            .map_err(|e| anyhow::anyhow!("解析 DBNUM 状态失败 dbnum={dbnum}: {e}"))?;
        let info_max: Option<i32> = response
            .take(1)
            .map_err(|e| anyhow::anyhow!("解析旧 DBNUM 水位失败 dbnum={dbnum}: {e}"))?;
        Ok((rows.into_iter().next(), info_max))
    }

    /// Read the effective state for one `dbnum` (with one-time migration applied
    /// in-memory). Returns `None` only when the `dbnum` has no record at all.
    pub async fn read(dbnum: u32) -> anyhow::Result<Option<DbnumState>> {
        let (row, info_max) = Self::read_row(dbnum).await?;
        let Some(row) = row else {
            // No dedicated record. Still surface a migrated watermark if the info
            // table knows one (legacy full-parse before the watermark existed).
            let applied = resolve_migrated_applied_sesno(None, None, info_max);
            return Ok(applied.map(|applied| DbnumState {
                dbnum,
                applied_sesno: applied,
                initialized: true,
                ..Default::default()
            }));
        };

        let applied = resolve_migrated_applied_sesno(row.applied_sesno, row.sesno, info_max);
        Ok(Some(DbnumState {
            dbnum: row.dbnum.unwrap_or(dbnum),
            db_type: row.db_type.unwrap_or_default(),
            file_name: row.file_name.unwrap_or_default(),
            file_path: row.file_path.unwrap_or_default(),
            file_size: row.file_size.unwrap_or_default(),
            file_latest_sesno: row.file_latest_sesno.unwrap_or_default(),
            applied_sesno: applied.unwrap_or_default(),
            initialized: applied.is_some(),
        }))
    }

    /// Authoritative applied watermark for one `dbnum` (0 when uninitialized).
    ///
    /// Read-only: never writes, so it is safe to call from preview scanning.
    pub async fn applied_sesno(dbnum: u32) -> anyhow::Result<i32> {
        let (row, info_max) = Self::read_row(dbnum).await?;
        let (existing_applied, legacy_sesno) = row
            .map(|r| (r.applied_sesno, r.sesno))
            .unwrap_or((None, None));
        Ok(resolve_migrated_applied_sesno(existing_applied, legacy_sesno, info_max).unwrap_or(0))
    }

    /// Persist a scan observation WITHOUT touching the applied watermark.
    ///
    /// Refreshes only the file-identity + observation fields and `scanned_at`
    /// (ADR-001: "预览扫描可以更新文件身份、属性、file_latest_sesno 和 scanned_at").
    /// `applied_sesno` is never written here, so preview scans leave the
    /// authoritative watermark unchanged; it is established durably only on the
    /// success path via [`Self::advance_applied`], while reads resolve it through
    /// the one-time migration in [`resolve_migrated_applied_sesno`].
    pub async fn record_scan(obs: &FileObservation) -> anyhow::Result<()> {
        let modified_expr = obs
            .file_modified_at
            .as_deref()
            .map(|s| format!("type::datetime('{}')", escape_surql_str(s)))
            .unwrap_or_else(|| "time::now()".to_string());

        let sql = format!(
            "UPSERT {WATERMARK_TABLE}:{dbnum} SET \
             dbnum = {dbnum}, db_type = '{db_type}', file_name = '{file_name}', \
             file_path = '{file_path}', file_size = {file_size}, \
             file_latest_sesno = {file_latest_sesno}, file_modified_at = {modified_expr}, \
             scanned_at = time::now(), updated_at = time::now();",
            dbnum = obs.dbnum,
            db_type = escape_surql_str(&obs.db_type),
            file_name = escape_surql_str(&obs.file_name),
            file_path = escape_surql_str(&obs.file_path),
            file_size = obs.file_size,
            file_latest_sesno = obs.file_latest_sesno,
        );
        SUL_DB
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("记录扫描观察失败 dbnum={}: {}", obs.dbnum, e))?
            .check()
            .map_err(|e| anyhow::anyhow!("记录扫描观察语句失败 dbnum={}: {}", obs.dbnum, e))?;
        Ok(())
    }

    /// 阻断类异常下的扫描落库：**只写观察值，不写文件身份**。
    ///
    /// `db_type` / `file_name` / `file_path` 是 [`check_file_against_state`] 的判据。
    /// 判为阻断（回退 / 类型变更 / …）时若照常 UPSERT 它们，登记基准就被现场的
    /// 那个文件顶掉了——下一轮再扫，`stored_db_type` 已经等于 `observed_db_type`，
    /// 同一个异常再也检不出来，异常把自己抹掉了。
    ///
    /// 观察值（大小、文件最新会话号、扫描时刻）照写：人从面板上仍要看得见
    /// 「现场那个文件长什么样」，这也是判断阻断是否已被人工处理掉的依据。
    /// 与 [`Self::record_scan`] 一样，永不触碰 `applied_sesno`（ADR-001）。
    pub async fn record_blocked_observation(obs: &FileObservation) -> anyhow::Result<()> {
        let sql = format!(
            "UPSERT {WATERMARK_TABLE}:{dbnum} SET dbnum = {dbnum}, \
             file_size = {file_size}, file_latest_sesno = {file_latest_sesno}, \
             scanned_at = time::now(), updated_at = time::now();",
            dbnum = obs.dbnum,
            file_size = obs.file_size,
            file_latest_sesno = obs.file_latest_sesno,
        );
        SUL_DB
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("记录阻断观察失败 dbnum={}: {}", obs.dbnum, e))?
            .check()
            .map_err(|e| anyhow::anyhow!("记录阻断观察语句失败 dbnum={}: {}", obs.dbnum, e))?;
        Ok(())
    }

    /// Advance the applied watermark for one `dbnum` after a data batch succeeds.
    ///
    /// Monotonic (`math::max`, never regresses) and only ever called on the success
    /// path. Mirrors the legacy `sesno` field for backward compatibility.
    pub async fn advance_applied(dbnum: u32, end_sesno: i32) -> anyhow::Result<()> {
        let sql = format!(
            "UPSERT {WATERMARK_TABLE}:{dbnum} SET dbnum = {dbnum}, \
             applied_sesno = math::max([applied_sesno?:0, {end_sesno}]), \
             sesno = math::max([sesno?:0, {end_sesno}]), \
             applied_at = time::now(), updated_at = time::now();"
        );
        SUL_DB
            .query(sql)
            .await
            .map_err(|e| anyhow::anyhow!("推进应用水位失败 dbnum={}: {}", dbnum, e))?
            .check()
            .map_err(|e| anyhow::anyhow!("推进应用水位语句失败 dbnum={}: {}", dbnum, e))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_prefers_established_applied_over_legacy_and_info() {
        // Already established → never re-mix other sources.
        assert_eq!(
            resolve_migrated_applied_sesno(Some(50), Some(99), Some(120)),
            Some(50)
        );
    }

    #[test]
    fn migration_inherits_legacy_watermark_when_not_established() {
        assert_eq!(
            resolve_migrated_applied_sesno(None, Some(99), Some(120)),
            Some(99)
        );
    }

    #[test]
    fn migration_falls_back_to_info_table_only_when_no_watermark() {
        assert_eq!(
            resolve_migrated_applied_sesno(None, None, Some(120)),
            Some(120)
        );
    }

    #[test]
    fn migration_none_when_all_sources_absent() {
        assert_eq!(resolve_migrated_applied_sesno(None, None, None), None);
    }

    #[test]
    fn migration_preserves_zero_applied() {
        // An established applied_sesno of 0 is a real value, not "absent".
        assert_eq!(
            resolve_migrated_applied_sesno(Some(0), Some(99), Some(120)),
            Some(0)
        );
    }

    /// `blocks()` 是阻断与否的唯一权威，自动路径与手动预览都读它，所以每一个变体
    /// 的取值都要在这里说死。用 `match` 而不是逐个 `assert!`：新增一种异常时这里
    /// 编译不过，作者必须显式选边，而不是让它默默落进「不阻断」。
    #[test]
    fn every_anomaly_declares_whether_it_blocks() {
        let cases = [
            FileAnomaly::Rollback {
                file_latest_sesno: 80,
                applied_sesno: 120,
            },
            FileAnomaly::PathMigrated {
                old_path: "/old".into(),
                new_path: "/new".into(),
            },
            FileAnomaly::TypeChanged {
                stored_db_type: "DESI".into(),
                observed_db_type: "CATA".into(),
            },
            FileAnomaly::Duplicate {
                paths: vec!["/a".into(), "/b".into()],
            },
            FileAnomaly::Missing {
                path: "/gone".into(),
            },
        ];
        for anomaly in &cases {
            let expected = match anomaly {
                // 良性搬家：登记路径跟着更新即可。
                FileAnomaly::PathMigrated { .. } => false,
                // 其余四种都动了「这个 dbnum 对应哪个文件」这件事的根基。
                FileAnomaly::Rollback { .. }
                | FileAnomaly::TypeChanged { .. }
                | FileAnomaly::Duplicate { .. }
                | FileAnomaly::Missing { .. } => true,
            };
            assert_eq!(anomaly.blocks(), expected, "{anomaly:?} 的阻断口径不符");
        }
    }

    #[test]
    fn file_rollback_is_rejected() {
        let anomaly = check_file_against_state(
            Some("DESI"),
            Some("/p/desi_1"),
            120,
            "DESI",
            "/p/desi_1",
            80,
        );
        assert_eq!(
            anomaly,
            Some(FileAnomaly::Rollback {
                file_latest_sesno: 80,
                applied_sesno: 120,
            })
        );
    }

    #[test]
    fn file_rollback_takes_precedence_over_path_change() {
        let anomaly = check_file_against_state(
            Some("DESI"),
            Some("/old/path"),
            120,
            "DESI",
            "/new/path",
            80,
        );
        assert!(matches!(anomaly, Some(FileAnomaly::Rollback { .. })));
    }

    #[test]
    fn legal_path_migration_is_detected() {
        let anomaly = check_file_against_state(
            Some("DESI"),
            Some("/old/path"),
            120,
            "DESI",
            "/new/path",
            130,
        );
        assert_eq!(
            anomaly,
            Some(FileAnomaly::PathMigrated {
                old_path: "/old/path".to_string(),
                new_path: "/new/path".to_string(),
            })
        );
    }

    #[test]
    fn db_type_change_is_blocked() {
        let anomaly = check_file_against_state(
            Some("DESI"),
            Some("/old/path"),
            120,
            "CATA",
            "/new/path",
            130,
        );
        assert_eq!(
            anomaly,
            Some(FileAnomaly::TypeChanged {
                stored_db_type: "DESI".to_string(),
                observed_db_type: "CATA".to_string(),
            })
        );
    }

    #[test]
    fn normal_file_reports_no_anomaly() {
        let anomaly = check_file_against_state(
            Some("DESI"),
            Some("/p/desi_1"),
            120,
            "DESI",
            "/p/desi_1",
            130,
        );
        assert_eq!(anomaly, None);
    }

    #[test]
    fn escape_handles_windows_paths_and_quotes() {
        assert_eq!(
            escape_surql_str(r"C:\proj\d'esi"),
            r"C:\\proj\\d\'esi".to_string()
        );
    }

    /// F6 · T605（live）：扫描观察落库，但**永不**触碰应用水位。
    ///
    /// 上面那批纯函数用例已经覆盖了 Rollback / PathMigrated 的判定口径；这里补的是
    /// 落库那一半：`record_scan` 只写文件身份与观察字段，`applied_sesno` 必须纹丝不动
    /// （ADR-001）。换成更旧的会话文件时同样不得让水位倒退，只把观察值照实记下来，
    /// 由调用方拿判定结果去阻断该 dbnum。
    ///
    /// 用 `999_999_001` 这个不会出现在真实工程里的 dbnum，跑完即清理。
    #[tokio::test]
    #[ignore = "manual live: requires the configured Surreal database"]
    async fn live_record_scan_never_moves_the_applied_watermark() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");

        let dbnum = 999_999_001u32;
        let cleanup = format!("delete {WATERMARK_TABLE}:{dbnum};");
        SUL_DB
            .query(&cleanup)
            .await
            .expect("clear stale fixture")
            .check()
            .expect("valid pre-cleanup");

        // 先把水位建立在 50——模拟这个 dbnum 已经成功应用到第 50 个会话。
        DbnumState::advance_applied(dbnum, 50)
            .await
            .expect("establish watermark");

        // 扫描到文件被移动且带来更新的会话：身份字段该更新，水位不该动。
        DbnumState::record_scan(&FileObservation {
            dbnum,
            db_type: "DESI".to_string(),
            file_name: "zz_t605.dbnum".to_string(),
            file_path: r"D:\zz_t605\moved\desi_1".to_string(),
            file_size: 4096,
            file_latest_sesno: 60,
            file_modified_at: None,
        })
        .await
        .expect("record moved-file scan");

        let moved = DbnumState::read(dbnum)
            .await
            .expect("read state after move")
            .expect("state exists");

        // 再扫到一个更旧的文件（回退观察）：观察值照实写，水位仍不得倒退。
        DbnumState::record_scan(&FileObservation {
            dbnum,
            db_type: "DESI".to_string(),
            file_name: "zz_t605.dbnum".to_string(),
            file_path: r"D:\zz_t605\moved\desi_1".to_string(),
            file_size: 2048,
            file_latest_sesno: 10,
            file_modified_at: None,
        })
        .await
        .expect("record rolled-back scan");

        let rolled_back = DbnumState::read(dbnum)
            .await
            .expect("read state after rollback")
            .expect("state exists");

        SUL_DB
            .query(&cleanup)
            .await
            .expect("cleanup fixture")
            .check()
            .expect("valid cleanup");

        assert_eq!(moved.file_path, r"D:\zz_t605\moved\desi_1");
        assert_eq!(moved.file_latest_sesno, 60);
        assert_eq!(moved.applied_sesno, 50, "预览扫描不得推进 applied_sesno");

        assert_eq!(
            rolled_back.file_latest_sesno, 10,
            "观察字段应如实记录更旧的文件"
        );
        assert_eq!(
            rolled_back.applied_sesno, 50,
            "回退文件不得让 applied_sesno 倒退"
        );

        // 判定口径与落库状态一致：这一观察应被判为 Rollback，由调用方阻断该 dbnum。
        assert!(matches!(
            check_file_against_state(
                Some("DESI"),
                Some(r"D:\zz_t605\moved\desi_1"),
                rolled_back.applied_sesno,
                "DESI",
                r"D:\zz_t605\moved\desi_1",
                rolled_back.file_latest_sesno,
            ),
            Some(FileAnomaly::Rollback { .. })
        ));
    }
}
