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
//! 3. Until the pe-based compatibility seeding has completed once on this
//!    database (durable `queue_control:watermark_seed` marker; also covers a
//!    missing or empty watermark table), establish each dbnum from the max
//!    persisted `pe.sesno`. Fill-only, so reruns never overwrite established
//!    watermarks.
//! 4. Otherwise (only when a dedicated row is absent) fall back once to the max
//!    `sesno` in `dbnum_info_table` for this `dbnum`.
//!
//! After the state is established (a scan / advance writes `applied_sesno`), reads
//! use `applied_sesno` directly and never re-mix other sources.

use std::collections::BTreeMap;

use aios_core::SUL_DB;
use serde::{Deserialize, Serialize};

/// Authoritative per-`dbnum` state table (extends the legacy watermark record).
pub const WATERMARK_TABLE: &str = "dbnum_watermark";
/// Legacy per-`ref_0` element-statistics table, used only for one-time migration.
pub const INFO_TABLE: &str = "dbnum_info_table";

const INCREMENT_STATE_SCHEMA: &str = r#"
DEFINE TABLE IF NOT EXISTS dbnum_watermark SCHEMALESS;
DEFINE TABLE IF NOT EXISTS dbnum_info_table SCHEMALESS;
DEFINE TABLE IF NOT EXISTS increment_update_attempt SCHEMALESS;
DEFINE TABLE IF NOT EXISTS model_update_pending SCHEMALESS;
DEFINE TABLE IF NOT EXISTS incr_side_effect_pending SCHEMALESS;
DEFINE TABLE IF NOT EXISTS queue_control SCHEMALESS;
"#;

const CURRENT_DATABASE_WATERMARKS: &str = "SELECT dbnum, math::max(sesno) AS sesno FROM pe \
     WHERE dbnum != NONE AND sesno != NONE GROUP BY dbnum;";
const LEGACY_INFO_WATERMARKS: &str = "SELECT dbnum, math::max(sesno) AS sesno FROM dbnum_info_table \
     WHERE dbnum != NONE AND sesno != NONE GROUP BY dbnum;";

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

#[derive(Debug, Deserialize)]
struct DatabaseWatermark {
    dbnum: u32,
    sesno: i32,
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

    /// 说给人听的阻断理由；不阻断的异常返回 `None`。
    ///
    /// 回退那句的措辞被 `docs/specs/web-service-api.md` 的回执样例钉着，别改。
    pub fn block_reason(&self) -> Option<String> {
        match self {
            FileAnomaly::PathMigrated { .. } => None,
            FileAnomaly::Rollback {
                file_latest_sesno,
                applied_sesno,
            } => Some(format!(
                "文件回退或被替换（file_latest_sesno={file_latest_sesno} < \
                 applied_sesno={applied_sesno}），已阻断"
            )),
            FileAnomaly::TypeChanged {
                stored_db_type,
                observed_db_type,
            } => Some(format!(
                "库类型变更（登记 {stored_db_type} → 现场 {observed_db_type}），已阻断"
            )),
            FileAnomaly::Duplicate { paths } => Some(format!(
                "同 dbnum 存在多个文件，已阻断: {}",
                paths.join("; ")
            )),
            FileAnomaly::Missing { path } => {
                Some(format!("登记文件缺失，已阻断: {path}"))
            }
        }
    }
}

/// 一次扫描观察的裁决：登记状态 + 异常分类。
///
/// 存在的理由是**落库口径只能有一处**。判据（`db_type` / `file_name` /
/// `file_path`）与写这几个字段的语句是同一批，谁先谁后、阻断时写不写，过去由
/// 每个调用点各自决定，于是自动路径用只写观察值的那条语句保住了证据，而手动
/// 预览与执行体照常刷新文件身份——同一个 `TypeChanged`，点一次预览就被自己
/// 抹掉，连自动路径下一轮也检不出来了。
///
/// 现在拿到裁决才能落库（[`DbnumState::record_observation`] 由裁决自己选语句），
/// 调用方剩下的自由只有「阻断了要说什么」。
#[derive(Debug, Clone)]
pub struct ScanVerdict {
    /// 本次观察之前登记的状态；`None` 表示这个 dbnum 从未登记过。
    pub prior: Option<DbnumState>,
    pub anomaly: Option<FileAnomaly>,
}

impl ScanVerdict {
    /// 权威水位（未登记时为 0）。
    pub fn applied_sesno(&self) -> i32 {
        self.prior.as_ref().map(|s| s.applied_sesno).unwrap_or(0)
    }

    /// 上一次观察到的文件最新会话号（未登记时为 0）。
    pub fn previous_file_latest_sesno(&self) -> i32 {
        self.prior
            .as_ref()
            .map(|s| s.file_latest_sesno)
            .unwrap_or(0)
    }

    pub fn blocked(&self) -> bool {
        self.anomaly.as_ref().is_some_and(FileAnomaly::blocks)
    }

    /// 阻断理由；没阻断时 `None`。
    pub fn block_reason(&self) -> Option<String> {
        self.anomaly.as_ref().and_then(FileAnomaly::block_reason)
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

fn database_has_table(info: &serde_json::Value, table: &str) -> bool {
    info.get("tables")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|tables| tables.contains_key(table))
}

/// 播种完成标记：pe 兼容播种在此库上**完整**跑完过一次。
///
/// 分块 UPSERT 每 500 条一批；部分批次成功后死掉，表里已经有行，「空表判定」
/// 就分不出「跑完了」与「跑了一半」。只有一个在全部批次成功之后才落库的标记
/// 能把两者区分开。与暂停旗标（`queue_control:main`）同表不同行。
const SEED_MARKER: &str = "queue_control:watermark_seed";

/// 是否需要按当前数据库（pe）播种兼容水位。
///
/// 三个条件任意一个成立就（重）跑 pe 源：表缺失、表为空、播种完成标记缺失。
/// 前两个是显而易见的初始/崩溃状态；第三个覆盖「分块 UPSERT 部分完成后死掉」
/// 与「从没跑过带标记版本的老库」——两者从表内容上无法区分，只能统一补跑。
/// 重跑无损：回填是 `??` 填空，绝不覆盖已建立水位，代价只是该库升级后的
/// 第一次启动多一次 pe 全表聚合（有日志提示）。
///
/// 若不这样判，后果是：pe 源本要救的那类老库（`dbnum_info_table` 缺失或陈旧）
/// 半途死一次后，下次启动切到 info 源，没播上的 dbnum 以 0 水位被
/// `needs_initial_load` 判成首次导入，整库全量重解析。
fn should_seed_from_current_database(
    watermark_table_missing: bool,
    watermark_rows: usize,
    seed_marker_present: bool,
) -> bool {
    watermark_table_missing || watermark_rows == 0 || !seed_marker_present
}

/// 播种完整性告警（纯函数）：pe 与统计表的按 dbnum 元素计数对不上。
///
/// 正常基线路径有 `count(pe) == sum(info.count)` 的完整性校验，播种路径没有——
/// 一个被历史全量解析中断留下洞的老库，pe 的最大会话号会接近文件尾，播种后
/// 增量永远补不回中间的洞。这里不阻断（播种照旧），只把嫌疑喊出来让人处置。
///
/// `dbnum_info_table` 整体为空（更老的库没有这张表）时无从比对，返回一条
/// 整体提示而不是逐库刷屏。
fn seed_integrity_warnings(
    pe_counts: &BTreeMap<u32, i64>,
    info_counts: &BTreeMap<u32, i64>,
) -> Vec<String> {
    if pe_counts.is_empty() {
        return Vec::new();
    }
    if info_counts.is_empty() {
        return vec![
            "播种完整性比对跳过：dbnum_info_table 无统计数据（更老的库没有这张表，属预期）"
                .to_string(),
        ];
    }
    pe_counts
        .iter()
        .filter(|(dbnum, pe_count)| {
            info_counts.get(dbnum).copied().unwrap_or(0) != **pe_count
        })
        .map(|(dbnum, pe_count)| {
            let info_count = info_counts.get(dbnum).copied().unwrap_or(0);
            format!(
                "播种完整性告警 dbnum={dbnum}：pe {pe_count} 条 != 统计 {info_count} 条；\
                 该库可能带着历史解析中断留下的洞，播种仍按 pe 最大会话号建立基线；\
                 如需彻底校验，请对该库重建基线（清空后走首次导入）"
            )
        })
        .collect()
}

fn migration_watermark_source(seed_from_current_database: bool) -> (&'static str, &'static str) {
    if seed_from_current_database {
        (CURRENT_DATABASE_WATERMARKS, "现有 PE 数据")
    } else {
        (LEGACY_INFO_WATERMARKS, "旧 DBNUM 统计")
    }
}

/// 现有水位行数（表不存在时 SurrealDB 对 SELECT 返回空集，得 0）。
async fn count_watermark_rows() -> anyhow::Result<usize> {
    #[derive(Deserialize)]
    struct CountRow {
        count: usize,
    }
    let mut response = SUL_DB
        .query(format!(
            "SELECT count() AS count FROM {WATERMARK_TABLE} GROUP ALL;"
        ))
        .await
        .map_err(|e| anyhow::anyhow!("统计增量水位行数失败: {e}"))?
        .check()
        .map_err(|e| anyhow::anyhow!("统计增量水位行数语句失败: {e}"))?;
    let rows: Vec<CountRow> = response
        .take(0)
        .map_err(|e| anyhow::anyhow!("解码增量水位行数失败: {e}"))?;
    Ok(rows.first().map(|row| row.count).unwrap_or_default())
}

/// 播种完成标记是否存在（表/行缺失时 SELECT 返回空集，得 `false`）。
async fn seed_marker_present() -> anyhow::Result<bool> {
    #[derive(Deserialize)]
    struct MarkerRow {
        #[serde(default)]
        dbnum_count: Option<i64>,
    }
    let mut response = SUL_DB
        .query(format!("SELECT dbnum_count FROM {SEED_MARKER};"))
        .await
        .map_err(|e| anyhow::anyhow!("读取播种完成标记失败: {e}"))?
        .check()
        .map_err(|e| anyhow::anyhow!("读取播种完成标记语句失败: {e}"))?;
    let rows: Vec<MarkerRow> = response
        .take(0)
        .map_err(|e| anyhow::anyhow!("解码播种完成标记失败: {e}"))?;
    Ok(!rows.is_empty())
}

/// 全部播种批次成功后落下完成标记。
async fn write_seed_marker(source_name: &str, seeded_dbnums: usize) -> anyhow::Result<()> {
    let sql = format!(
        "UPSERT {SEED_MARKER} SET source = '{source_name}', \
         dbnum_count = {seeded_dbnums}, completed_at = time::now();"
    );
    SUL_DB
        .query(sql)
        .await
        .map_err(|e| anyhow::anyhow!("写播种完成标记失败: {e}"))?
        .check()
        .map_err(|e| anyhow::anyhow!("写播种完成标记语句失败: {e}"))?;
    Ok(())
}

/// 拉取播种完整性比对的两组按 dbnum 计数：pe 实际元素数、`dbnum_info_table` 统计和。
///
/// pe 侧是第二次全表扫描（与水位聚合分开发，互不影响已验证的水位语句）；
/// 只在走 pe 源播种的启动里执行一次。
async fn fetch_seed_integrity_counts()
-> anyhow::Result<(BTreeMap<u32, i64>, BTreeMap<u32, i64>)> {
    #[derive(Deserialize)]
    struct DbnumCount {
        dbnum: u32,
        count: i64,
    }
    let mut response = SUL_DB
        .query("SELECT dbnum, count() AS count FROM pe WHERE dbnum != NONE GROUP BY dbnum;")
        .query(
            "SELECT dbnum, math::sum(count) AS count FROM dbnum_info_table \
             WHERE dbnum != NONE GROUP BY dbnum;",
        )
        .await
        .map_err(|e| anyhow::anyhow!("读取播种完整性计数失败: {e}"))?
        .check()
        .map_err(|e| anyhow::anyhow!("读取播种完整性计数语句失败: {e}"))?;
    let pe_rows: Vec<DbnumCount> = response
        .take(0)
        .map_err(|e| anyhow::anyhow!("解码 pe 元素计数失败: {e}"))?;
    let info_rows: Vec<DbnumCount> = response
        .take(1)
        .map_err(|e| anyhow::anyhow!("解码 DBNUM 统计计数失败: {e}"))?;
    Ok((
        pe_rows.into_iter().map(|r| (r.dbnum, r.count)).collect(),
        info_rows.into_iter().map(|r| (r.dbnum, r.count)).collect(),
    ))
}

impl DbnumState {
    /// Ensure an old Surreal database can run the current incremental pipeline in place.
    ///
    /// Table definitions are idempotent. When the watermark table is absent — or exists but
    /// holds no rows yet (a previous run died between table creation and seeding) — this is
    /// an in-place compatibility upgrade: the durable baseline comes from the maximum `sesno`
    /// already persisted in `pe` for each `dbnum`. Watermark tables with established rows
    /// keep their own established/legacy watermarks and use `dbnum_info_table` only as the
    /// old fallback.
    pub async fn ensure_increment_state_storage() -> anyhow::Result<usize> {
        let mut response = SUL_DB
            .query("INFO FOR DB;")
            .await
            .map_err(|e| anyhow::anyhow!("检查增量状态表失败: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("检查增量状态表语句失败: {e}"))?;
        let database_info: Option<serde_json::Value> = response
            .take(0)
            .map_err(|e| anyhow::anyhow!("解码数据库表信息失败: {e}"))?;
        let database_info = database_info
            .ok_or_else(|| anyhow::anyhow!("数据库表信息为空，无法安全初始化增量状态"))?;
        let watermark_table_missing = !database_has_table(&database_info, WATERMARK_TABLE);
        // 行数与完成标记必须在建表与任何写入之前取：它们共同决定播种源。
        let watermark_rows = if watermark_table_missing {
            0
        } else {
            count_watermark_rows().await?
        };
        let marker_present = seed_marker_present().await?;
        let seed_from_current_database =
            should_seed_from_current_database(watermark_table_missing, watermark_rows, marker_present);

        SUL_DB
            .query(INCREMENT_STATE_SCHEMA)
            .await
            .map_err(|e| anyhow::anyhow!("初始化增量状态表失败: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("初始化增量状态表语句失败: {e}"))?;

        SUL_DB
            .query(
                "UPDATE dbnum_watermark SET applied_sesno = sesno \
                 WHERE applied_sesno = NONE AND sesno != NONE;",
            )
            .await
            .map_err(|e| anyhow::anyhow!("迁移旧 DBNUM 水位失败: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("迁移旧 DBNUM 水位语句失败: {e}"))?;

        let (source_sql, source_name) = migration_watermark_source(seed_from_current_database);
        if seed_from_current_database {
            // 这一步在大库上是 pe 全表聚合，可能长时间无输出；不喊一声的话，
            // 现场很容易把首次兼容启动当成卡死。
            let reason = if watermark_table_missing {
                "水位表缺失"
            } else if watermark_rows == 0 {
                "水位表为空"
            } else {
                "播种完成标记缺失（上次播种中断，或首次升级到带标记的版本）"
            };
            println!(
                "增量水位播种开始：{reason}，按{source_name}（每个 dbnum 的最大 sesno）建立基线，大库上可能耗时较长…"
            );
            // 完整性嫌疑要在播种前喊出来：告警不阻断，播种照旧。
            match fetch_seed_integrity_counts().await {
                Ok((pe_counts, info_counts)) => {
                    for warning in seed_integrity_warnings(&pe_counts, &info_counts) {
                        eprintln!("{warning}");
                    }
                }
                Err(error) => {
                    eprintln!("播种完整性比对失败（不阻断播种）: {error:#}");
                }
            }
        }
        let seed_started = std::time::Instant::now();
        let mut response = SUL_DB
            .query(source_sql)
            .await
            .map_err(|e| anyhow::anyhow!("读取{source_name}水位失败: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("读取{source_name}水位语句失败: {e}"))?;
        let watermarks: Vec<DatabaseWatermark> = response
            .take(0)
            .map_err(|e| anyhow::anyhow!("解码{source_name}水位失败: {e}"))?;

        for chunk in watermarks.chunks(500) {
            let sql = chunk
                .iter()
                .map(|row| {
                    format!(
                        "UPSERT {WATERMARK_TABLE}:{dbnum} SET dbnum = {dbnum}, \
                         applied_sesno = applied_sesno ?? {sesno}, \
                         sesno = sesno ?? {sesno}, updated_at = time::now();",
                        dbnum = row.dbnum,
                        sesno = row.sesno,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            SUL_DB
                .query(sql)
                .await
                .map_err(|e| anyhow::anyhow!("固化{source_name}水位失败: {e}"))?
                .check()
                .map_err(|e| anyhow::anyhow!("固化{source_name}水位语句失败: {e}"))?;
        }
        if seed_from_current_database {
            // 全部批次成功才落标记；写失败只提示不阻断——后果不过是下次启动
            // 重跑一遍 fill-only 播种，幂等无损。
            if let Err(error) = write_seed_marker(source_name, watermarks.len()).await {
                eprintln!("写播种完成标记失败（下次启动会重新播种一遍，幂等无损）: {error:#}");
            }
        }
        println!(
            "增量水位回填检查完成：源={source_name}，来源 {} 个 dbnum，耗时 {:.1} 秒",
            watermarks.len(),
            seed_started.elapsed().as_secs_f32()
        );
        Ok(watermarks.len())
    }

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

    /// 读登记身份并对一次观察下裁决（纯读，不写任何东西）。
    ///
    /// 这是全仓判定文件异常的唯一入口：读状态、比判据、分类，一次做完，谁都不用
    /// 再自己拼 [`check_file_against_state`] 的六个参数——过去四个调用点各拼一遍，
    /// 其中两个干脆忘了拼。
    ///
    /// 读失败**必须上浮**，不能吞成「从未登记」。吞掉的话 `applied_sesno` 退化成 0，
    /// 回退检不出来，`needs_initial_load` 还会判成「首次导入」——一次数据库抖动就能
    /// 让一个跑了很久的库被当成新库重新全量解析。各调用方自己决定是跳过还是失败。
    pub async fn classify_scan(obs: &FileObservation) -> anyhow::Result<ScanVerdict> {
        let prior = Self::read(obs.dbnum).await?;
        let anomaly = check_file_against_state(
            prior
                .as_ref()
                .map(|s| s.db_type.as_str())
                .filter(|s| !s.is_empty()),
            prior
                .as_ref()
                .map(|s| s.file_path.as_str())
                .filter(|s| !s.is_empty()),
            prior.as_ref().map(|s| s.applied_sesno).unwrap_or(0),
            &obs.db_type,
            &obs.file_path,
            obs.file_latest_sesno,
        );
        Ok(ScanVerdict { prior, anomaly })
    }

    /// 按裁决落库这次观察：阻断只写观察值，否则连文件身份一并刷新。
    ///
    /// 这是落库观察的**唯一入口**——底下那两条语句都是私有的，模块外拿不到。
    /// 「选哪条」不能有第二个决定点：选错一次，异常就把自己抹掉，而且是静默的
    /// ——下一轮扫描一切正常，只是那个库再也不会被拦下来了。
    pub async fn record_observation(
        obs: &FileObservation,
        verdict: &ScanVerdict,
    ) -> anyhow::Result<()> {
        if verdict.blocked() {
            Self::record_blocked_observation(obs).await
        } else {
            Self::record_scan(obs).await
        }
    }

    /// 测试夹具专用：无条件写入文件身份，绕开裁决。
    ///
    /// 只给「需要把某个身份摆进库里当前置条件」的 live 测试用（例如故意重放一个
    /// 更旧的基线，那种观察会被判成回退，走正门就写不进身份）。名字取得这么长
    /// 是故意的：生产代码里出现它，评审一眼就能看见。
    #[cfg(test)]
    pub(crate) async fn force_scan_identity_for_test(
        obs: &FileObservation,
    ) -> anyhow::Result<()> {
        Self::record_scan(obs).await
    }

    /// Persist a scan observation WITHOUT touching the applied watermark.
    ///
    /// Refreshes only the file-identity + observation fields and `scanned_at`
    /// (ADR-001: "预览扫描可以更新文件身份、属性、file_latest_sesno 和 scanned_at").
    /// `applied_sesno` is never written here, so preview scans leave the
    /// authoritative watermark unchanged; it is established durably only on the
    /// success path via [`Self::advance_applied`], while reads resolve it through
    /// the one-time migration in [`resolve_migrated_applied_sesno`].
    ///
    /// 私有：外面只能经 [`Self::record_observation`] 落库，由裁决决定走这条还是
    /// 只写观察值的那条。
    async fn record_scan(obs: &FileObservation) -> anyhow::Result<()> {
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
    /// 与 `record_scan` 一样，永不触碰 `applied_sesno`（ADR-001）。
    ///
    /// 私有：外面只能经 [`Self::record_observation`] 落库。
    async fn record_blocked_observation(obs: &FileObservation) -> anyhow::Result<()> {
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
    fn startup_schema_covers_every_increment_state_table() {
        for table in [
            WATERMARK_TABLE,
            INFO_TABLE,
            "increment_update_attempt",
            "model_update_pending",
            "incr_side_effect_pending",
            "queue_control",
        ] {
            assert!(
                INCREMENT_STATE_SCHEMA.contains(&format!("IF NOT EXISTS {table} ")),
                "missing startup definition for {table}"
            );
        }
    }

    #[test]
    fn missing_watermark_table_uses_current_pe_sessions_for_compatibility() {
        let before = serde_json::json!({"tables": {"pe": "DEFINE TABLE pe SCHEMALESS"}});
        let after = serde_json::json!({
            "tables": {
                "pe": "DEFINE TABLE pe SCHEMALESS",
                "dbnum_watermark": "DEFINE TABLE dbnum_watermark SCHEMALESS"
            }
        });

        let missing = !database_has_table(&before, WATERMARK_TABLE);
        let existing = !database_has_table(&after, WATERMARK_TABLE);
        let seed = should_seed_from_current_database(missing, 0, false);
        let keep_legacy = should_seed_from_current_database(existing, 7, true);
        assert!(migration_watermark_source(seed).0.contains("FROM pe"));
        assert!(migration_watermark_source(seed).0.contains("GROUP BY dbnum"));
        assert!(migration_watermark_source(keep_legacy)
            .0
            .contains("FROM dbnum_info_table"));
    }

    /// 崩溃窗口：上一轮在建表之后、播种完成之前死掉。空表（一批都没写上）与
    /// 半途表（部分批次写上了，行数 > 0 但完成标记没落）都必须继续走 pe 源；
    /// 只有「有行且标记在」的库才算播种完成、回到 info 源快路径。
    #[test]
    fn an_interrupted_seed_resumes_from_current_pe_data() {
        // 表缺失：无条件走 pe 源。
        assert!(should_seed_from_current_database(true, 0, false));
        // 表存在但一行都没有：建表后第一批就没写上。
        assert!(should_seed_from_current_database(false, 0, false));
        // 部分批次写上了（行数 > 0）但完成标记缺失：继续补跑 pe 源。
        assert!(should_seed_from_current_database(false, 500, false));
        // 有行且标记在：播种完成，走 info 源快路径。
        assert!(!should_seed_from_current_database(false, 1, true));
        assert!(!should_seed_from_current_database(false, 300, true));
        // 标记在但表被清空/重建：行数为 0 优先，重新播种（随后标记会被重写）。
        assert!(should_seed_from_current_database(false, 0, true));
        // 标记在但整张表被删了：缺表优先，重新播种。
        assert!(should_seed_from_current_database(true, 0, true));
    }

    /// 完整性告警只喊「对不上」的库：pe 与统计一致的不出声；统计缺行按 0 比；
    /// 统计表整体为空（更老的库没有它）时只给一条整体提示，不逐库刷屏。
    #[test]
    fn seed_integrity_flags_only_mismatched_dbnums() {
        let pe = BTreeMap::from([(7997u32, 1000i64), (8000, 34), (8191, 169)]);
        let info = BTreeMap::from([(7997u32, 1000i64), (8000, 30), (251047, 6)]);

        let warnings = seed_integrity_warnings(&pe, &info);
        // 7997 一致不告警；8000 计数不等、8191 统计缺行（按 0 比）要告警。
        assert_eq!(warnings.len(), 2);
        assert!(warnings.iter().any(|w| w.contains("dbnum=8000")
            && w.contains("pe 34 条")
            && w.contains("统计 30 条")));
        assert!(warnings
            .iter()
            .any(|w| w.contains("dbnum=8191") && w.contains("统计 0 条")));

        // 统计表整体为空：一条整体提示。
        let empty_info = BTreeMap::new();
        let skipped = seed_integrity_warnings(&pe, &empty_info);
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("跳过"));

        // pe 为空（全新库）：什么都不喊。
        assert!(seed_integrity_warnings(&BTreeMap::new(), &info).is_empty());
    }

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

    /// spec 001 · US2（live）：阻断落库只写观察值，**判据字段纹丝不动**。
    ///
    /// 这是那个 bug 的核心：`record_scan` 按 dbnum UPSERT `db_type` / `file_path`，
    /// 而它们正是 `check_file_against_state` 的判据。阻断时若照常写，第二轮扫描读到的
    /// `stored_db_type` 已经等于观察值，`TypeChanged` 再也检不出来——异常把自己抹掉了。
    /// 所以这条测试的重点不在第一次的返回值，而在**第二轮还能不能检出同一个异常**。
    ///
    /// 用 `999_999_002` 这个不会出现在真实工程里的 dbnum，跑完即清理。
    /// 空库即可验证，不需要解析过的 E3D 工程。
    #[tokio::test]
    #[ignore = "manual live: requires a reachable SurrealDB at the configured endpoint"]
    async fn live_blocked_observation_keeps_the_verdict_evidence_intact() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");

        let dbnum = 999_999_002u32;
        let cleanup = format!("delete {WATERMARK_TABLE}:{dbnum};");
        SUL_DB
            .query(&cleanup)
            .await
            .expect("clear stale fixture")
            .check()
            .expect("valid pre-cleanup");

        // 建立登记身份：DESI，某个路径，水位 50。
        DbnumState::advance_applied(dbnum, 50)
            .await
            .expect("establish watermark");
        DbnumState::record_scan(&FileObservation {
            dbnum,
            db_type: "DESI".to_string(),
            file_name: "zz_us2_0001".to_string(),
            file_path: r"D:\zz_us2\ams000\zz_us2_0001".to_string(),
            file_size: 4096,
            file_latest_sesno: 60,
            file_modified_at: None,
        })
        .await
        .expect("record the registered identity");

        // 现场文件换成了另一种类型的库 —— 这一观察应判 TypeChanged 且阻断。
        let observed_path = r"D:\zz_us2\cata000\zz_us2_0001".to_string();
        let anomaly = check_file_against_state(
            Some("DESI"),
            Some(r"D:\zz_us2\ams000\zz_us2_0001"),
            50,
            "CATA",
            &observed_path,
            70,
        );
        assert!(
            anomaly.as_ref().is_some_and(FileAnomaly::blocks),
            "前提：类型变更必须是阻断类异常，实际 {anomaly:?}"
        );

        // 阻断路径的落库。
        DbnumState::record_blocked_observation(&FileObservation {
            dbnum,
            db_type: "CATA".to_string(),
            file_name: "zz_us2_0001".to_string(),
            file_path: observed_path.clone(),
            file_size: 8192,
            file_latest_sesno: 70,
            file_modified_at: None,
        })
        .await
        .expect("record blocked observation");

        let after = DbnumState::read(dbnum)
            .await
            .expect("read state after blocked scan")
            .expect("state exists");

        // 第二轮：拿库里现存的登记身份再判一次，必须仍然是 TypeChanged。
        let second_round = check_file_against_state(
            Some(&after.db_type),
            Some(&after.file_path),
            after.applied_sesno,
            "CATA",
            &observed_path,
            70,
        );

        SUL_DB
            .query(&cleanup)
            .await
            .expect("cleanup fixture")
            .check()
            .expect("valid cleanup");

        // 判据字段：一个都不许动。
        assert_eq!(after.db_type, "DESI", "阻断不得改写登记的库类型");
        assert_eq!(
            after.file_path, r"D:\zz_us2\ams000\zz_us2_0001",
            "阻断不得改写登记的文件路径"
        );
        assert_eq!(after.file_name, "zz_us2_0001", "阻断不得改写登记的文件名");
        // 观察值：照实更新，人要从面板上看得见现场是什么样。
        assert_eq!(after.file_size, 8192);
        assert_eq!(after.file_latest_sesno, 70);
        // 水位：永不因为一次扫描而移动（ADR-001）。
        assert_eq!(after.applied_sesno, 50);
        // 而这才是重点：异常没有把自己抹掉。
        assert!(
            matches!(second_round, Some(FileAnomaly::TypeChanged { .. })),
            "第二轮必须仍能检出同一个异常，实际 {second_round:?}"
        );
    }

    /// 手动入队与 worker 执行体都是 `if let Some(reason) = verdict.block_reason()`
    /// 才拦——所以「阻断」与「有话说」必须严格同步。一个阻断类异常若返回 `None`，
    /// 那两处会一声不吭地把它放过去，恰好是本次要修的那类洞。
    #[test]
    fn every_blocking_anomaly_says_why_and_only_those() {
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
                observed_db_type: "SYST".into(),
            },
            FileAnomaly::Duplicate {
                paths: vec!["/a".into(), "/b".into()],
            },
            FileAnomaly::Missing {
                path: "/gone".into(),
            },
        ];
        for anomaly in &cases {
            assert_eq!(
                anomaly.block_reason().is_some(),
                anomaly.blocks(),
                "{anomaly:?}：阻断与理由必须同时有或同时无"
            );
        }
    }

    /// 回退那句的措辞被 `docs/specs/web-service-api.md` 的回执样例钉着。
    #[test]
    fn the_rollback_reason_matches_the_published_receipt_wording() {
        let reason = FileAnomaly::Rollback {
            file_latest_sesno: 812,
            applied_sesno: 1005,
        }
        .block_reason()
        .expect("回退是阻断类异常");
        assert_eq!(
            reason,
            "文件回退或被替换（file_latest_sesno=812 < applied_sesno=1005），已阻断"
        );
    }

    /// 从未登记过的库：水位与上一次观察都取 0，且不算异常——首次导入走的是
    /// `needs_initial_load`，不是阻断。
    #[test]
    fn an_unregistered_dbnum_yields_a_clean_verdict() {
        let verdict = ScanVerdict {
            prior: None,
            anomaly: None,
        };
        assert_eq!(verdict.applied_sesno(), 0);
        assert_eq!(verdict.previous_file_latest_sesno(), 0);
        assert!(!verdict.blocked());
        assert!(verdict.block_reason().is_none());
    }

    /// 路径迁移是良性搬家：不阻断，所以调用方照常执行，而落库会走刷新身份那条
    /// 语句把登记路径更新过来。
    #[test]
    fn a_migrated_path_does_not_block_the_batch() {
        let verdict = ScanVerdict {
            prior: Some(DbnumState {
                dbnum: 8000,
                db_type: "DESI".into(),
                file_path: "/old/desi_1".into(),
                file_latest_sesno: 120,
                applied_sesno: 120,
                initialized: true,
                ..Default::default()
            }),
            anomaly: Some(FileAnomaly::PathMigrated {
                old_path: "/old/desi_1".into(),
                new_path: "/new/desi_1".into(),
            }),
        };
        assert_eq!(verdict.applied_sesno(), 120);
        assert_eq!(verdict.previous_file_latest_sesno(), 120);
        assert!(!verdict.blocked());
        assert!(verdict.block_reason().is_none());
    }
}
