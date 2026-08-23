use aios_core::SUL_DB;
use aios_core::aios_db_mgr::aios_mgr::AiosDBMgr;
use aios_core::get_default_pdms_db_info;
use aios_core::helper::normalize_sql_string;
use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use aios_core::tool::db_tool::db1_dehash;
use aios_core::tool::hash_tool::hash_str;
use aios_core::types::*;
use anyhow::Context;
use chrono::Local;
use dashmap::{DashMap, DashSet};
use futures::StreamExt;
use futures::channel::mpsc::unbounded;
use futures::stream::FuturesUnordered;
use itertools::Itertools;
use parse_pdms_db::parse::*;
use pdms_io::io::PdmsIO;
use pe::SPdmsElement;
use petgraph::prelude::DiGraph;
use serde::Deserialize;
#[cfg(feature = "sql")]
use sqlx::{Connection, MySql, MySqlPool, Pool};
#[cfg(feature = "sql")]
use sqlx::{Error, Executor};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::hash::Hash;
use std::io::Read;
use std::mem::take;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use tokio::fs;
use tokio::fs::{File, create_dir_all};
use tokio::io::AsyncReadExt;
// use tokio::sync::mpsc::Sender;
use std::sync::mpsc::Sender;
use tokio::time::Instant;

use crate::consts::*;
use crate::data_interface::tidb_manager::AiosDBManager;
// use crate::graph_db::pdms_arango::*;
use crate::surreal_retry::retry_surreal_write_operation;
use crate::tables::*;
use crate::versioned_db::member_prune;
use crate::versioned_db::pe::*;
use crate::versioned_db::task::get_global_db_sender;

const BASELINE_QUEUE_CAPACITY: usize = 100;
const BASELINE_WRITE_WINDOW: usize = 20;
// SurrealDB 2.1/RocksDB uses optimistic transactions for these multi-row
// INSERTs. Even disjoint PE ids update shared table/index state, so concurrent
// baseline writers can keep colliding until the bounded retry budget expires.
// Keep parsing concurrent and the channel bounded, but serialize persistence.
const BASELINE_WRITE_WORKERS: usize = 1;

pub enum SenderJsonsData {
    PEJson(Vec<String>),
    PERelateJson(Vec<String>),
    AttJson((String, Vec<String>)),
    // 项目名 , sql
    MysqlSql((String, String)),
}

fn is_retryable_surreal_write_error(error: &str) -> bool {
    error.contains("read or write conflict") || error.contains("transaction can be retried")
}

/// 冲突重试的等待时长。
///
/// BASELINE_WRITE_WORKERS 个写入器争同一张表时冲突是持续的：固定或线性退避会让几个
/// 批次同步重试、一起再撞上，很快耗尽重试预算。指数增长拉开重试窗口，抖动把并发的
/// 写入器错开。
fn conflict_retry_backoff(attempt: usize) -> std::time::Duration {
    const BASE_MS: u64 = 25;
    const MAX_SHIFT: usize = 6;
    let backoff_ms = BASE_MS << attempt.min(MAX_SHIFT);
    let jitter_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos() as u64)
        .unwrap_or_default()
        % backoff_ms;
    std::time::Duration::from_millis(backoff_ms + jitter_ms)
}

/// 列出项目 `*000` 目录下需要解析的库文件。
///
/// 只保留普通文件：目录名不含 `.` 时会混进来，而 Windows 上 `File::open` 打开目录
/// 返回 PermissionDenied，会让整个解析任务 panic。抽取家族归并后只保留叶子
/// （主库被遮蔽）；同号兄弟抽取拒绝静默挑选。
/// `explicit_files`：调用方点名要解析的文件名（`included_db_files` 口径）。抽取家族
/// 归并会把被叶子 shadow 的主库从清单里删掉；但父层补缺（ADR-028 第 6 条）恰恰要
/// 点名解析主库——被点名的 shadow 文件必须回到清单，否则补缺同步静默空转。
fn collect_project_db_files(
    project_dir: &Path,
    explicit_files: Option<&[String]>,
) -> anyhow::Result<Vec<PathBuf>> {
    let target_dir = std::fs::read_dir(project_dir)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .find(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with("000"))
        })
        .ok_or_else(|| {
            anyhow::anyhow!("项目目录下没有 *000 数据库目录: {}", project_dir.display())
        })?;

    let mut numbered = Vec::new();
    let mut passthrough = Vec::new();
    for path in std::fs::read_dir(target_dir)?.filter_map(|entry| entry.ok().map(|e| e.path())) {
        if !path.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if file_name.contains('.') {
            continue;
        }
        match crate::data_interface::extract_family::parse_extract_file_name(file_name) {
            Some(parsed) => numbered.push((".".to_string(), parsed.dbnum, path)),
            None => passthrough.push(path),
        }
    }
    let collapsed = crate::data_interface::extract_family::collapse_extract_families(numbered);
    if !collapsed.duplicate_keys.is_empty() {
        anyhow::bail!(
            "项目目录存在同号兄弟抽取，拒绝静默选一份: {:?}",
            collapsed.duplicate_keys
        );
    }
    let mut files: Vec<PathBuf> = collapsed
        .selected
        .into_iter()
        .map(|family| family.leaf_path)
        .collect();
    if let Some(explicit) = explicit_files {
        for parent in collapsed.shadowed_parents {
            let named = parent
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| explicit.iter().any(|want| want == name));
            if named {
                files.push(parent);
            }
        }
    }
    files.extend(passthrough);
    Ok(files)
}

/// 收尾写入管线：先关闭 sender、排空 writer 任务，再决定这次同步的成败。
///
/// 顺序是关键。解析任务失败时如果提前返回，已经解析好的数据会连同没人消费的通道
/// 一起被丢弃，日志却停在「开始保存pe数量」，看起来像成功。
async fn finish_write_pipeline(
    sender: flume::Sender<SenderJsonsData>,
    mut insert_handles: FuturesUnordered<tokio::task::JoinHandle<(usize, Vec<String>)>>,
    parser_outcome: anyhow::Result<()>,
) -> anyhow::Result<()> {
    drop(sender);
    let mut write_error_count = 0usize;
    let mut write_error_samples: Vec<String> = Vec::new();
    while let Some(result) = insert_handles.next().await {
        match result {
            Ok((count, samples)) => {
                write_error_count += count;
                for sample in samples {
                    if write_error_samples.len() < 3 {
                        write_error_samples.push(sample);
                    }
                }
            }
            Err(error) => {
                write_error_count += 1;
                if write_error_samples.len() < 3 {
                    write_error_samples.push(format!("writer task join failed: {error}"));
                }
            }
        }
    }
    parser_outcome?;
    if write_error_count > 0 {
        anyhow::bail!(
            "baseline SurrealDB write failed {write_error_count} time(s); samples: {}",
            write_error_samples.join("; ")
        );
    }
    Ok(())
}

fn baseline_cleanup_targets(
    failed_dbnums: impl IntoIterator<Item = u32>,
    scheduled_dbnums: impl IntoIterator<Item = u32>,
    pipeline_failed: bool,
) -> BTreeSet<u32> {
    let mut targets = failed_dbnums.into_iter().collect::<BTreeSet<_>>();
    if pipeline_failed {
        // writer 错误来自共享通道，当前消息没有可靠 dbnum 归因。为防止任一库留下
        // 部分基线，失败时保守清理本批所有已经开始调度写入的库。
        targets.extend(scheduled_dbnums);
    }
    targets
}

async fn execute_surreal_checked(sql: &str, context: &str) -> anyhow::Result<()> {
    const MAX_ATTEMPTS: usize = 16;
    for attempt in 1..=MAX_ATTEMPTS {
        let result = async {
            SUL_DB
                .query(sql)
                .await
                .map_err(|error| anyhow::anyhow!("{context} transport failed: {error}"))?
                .check()
                .map_err(|error| anyhow::anyhow!("{context} statement failed: {error}"))?;
            Ok::<(), anyhow::Error>(())
        }
        .await;
        match result {
            Ok(()) => return Ok(()),
            Err(error)
                if attempt < MAX_ATTEMPTS
                    && is_retryable_surreal_write_error(&error.to_string()) =>
            {
                tokio::time::sleep(conflict_retry_backoff(attempt)).await;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("bounded retry loop always returns")
}

fn record_write_error(
    total: &mut usize,
    samples: &mut Vec<String>,
    context: &str,
    error: anyhow::Error,
) {
    *total += 1;
    if samples.len() < 3 {
        samples.push(format!("{context}: {error}"));
    }
}

/// Keep the structural PE row when the full attribute decoder rejects one indexed element.
///
/// `parse_file_db_basic_data` has already validated the record index and extracted its noun plus
/// record-level owner. Dropping the row here makes every live descendant's persisted `owner` chain
/// dangle, so later incremental model generation cannot calculate `anc`. The fallback is
/// deliberately PE-only: it supplies identity/topology metadata without pretending that the
/// element's failed explicit attributes were decoded successfully.
fn preserve_unparsed_pe_metadata(
    db_basic: &aios_core::db::DbBasicData,
    chunk_refnos: &[RefU64],
    ses_range_map: &BTreeMap<i32, std::ops::Range<u32>>,
    total_attr_map: &DashMap<RefU64, NamedAttrMap>,
) -> Vec<RefU64> {
    let mut preserved = Vec::new();
    for &refno in chunk_refnos {
        if total_attr_map.contains_key(&refno) {
            continue;
        }
        let Some(entry) = db_basic.refno_table_map.get(&refno) else {
            continue;
        };
        let pos = entry.pos;
        if pos < 4 || pos + 20 > db_basic.bytes.len() {
            continue;
        }

        let owner = RefU64::from(&db_basic.bytes[pos + 12..pos + 20]);
        let noun = db_basic.get_type(refno);
        let pgno = (pos / 0x800) as u32;
        let sesno = ses_range_map
            .iter()
            .find_map(|(sesno, range)| range.contains(&pgno).then_some(*sesno))
            .unwrap_or_default();
        let mut attributes = NamedAttrMap::default();
        attributes
            .map
            .insert("TYPE".to_string(), NamedAttrValue::StringType(noun));
        attributes
            .map
            .insert("REFNO".to_string(), NamedAttrValue::RefU64Type(refno));
        attributes
            .map
            .insert("OWNER".to_string(), NamedAttrValue::RefU64Type(owner));
        attributes.set_sesno(sesno);
        total_attr_map.insert(refno, attributes);
        preserved.push(refno);
    }
    preserved
}

/// 按 Ref0 聚合一个 dbnum 的 pe 统计。**回归测试与生产共用这一份文本。**
///
/// 返回行数等于 Ref0 个数，与库里有多少 pe 行无关——这正是它存在的理由，
/// 见 [`rebuild_dbnum_info_from_pe`] 的注释。
fn pe_stat_groups_sql(dbnum: u32) -> String {
    format!(
        "SELECT string::split(record::id(id), '_')[0] AS ref0, count() AS count, \
         math::max(sesno ?? 0) AS max_sesno, \
         math::max(<int> string::split(record::id(id), '_')[1]) AS max_ref1 \
         FROM pe WHERE dbnum = {dbnum} GROUP BY ref0;"
    )
}

/// 只补 [`dbnum_event_sql`] 写不出的身份字段，不碰 count / sesno。
///
/// 与重建的关键差别是**没有 DELETE**：事件维护出来的那条行原地留着。
fn stamp_dbnum_info_identity_sql(dbnum: u32, file_name: &str, db_type: &str) -> String {
    let file_name = file_name.replace('\'', "\\'");
    let db_type = db_type.replace('\'', "\\'");
    format!(
        "UPDATE dbnum_info_table SET file_name = '{file_name}', db_type = '{db_type}' \
         WHERE dbnum = {dbnum};"
    )
}

/// total sync 结尾要不要付全量重算的代价。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatsSettlement {
    /// 统计已由事件维护到位，只补身份字段。
    StampIdentity,
    /// 统计缺席或对不上，从 pe 全量重算。
    Rebuild,
}

/// 纯判定：两侧计数对得上、且统计行确实存在，才算「事件已经维护到位」。
///
/// `info_rows > 0` 这一半不能省——统计整体缺席时两侧的和同为 0（空库，或
/// `sync_sys_only` 摘着事件写出来的那批行），`pe_count == info_count` 会成立，
/// 但那不是「维护到位」，是「一条都没记」。
fn classify_stats_settlement(
    pe_count: usize,
    info_rows: usize,
    info_count: usize,
) -> StatsSettlement {
    if info_rows > 0 && pe_count == info_count {
        StatsSettlement::StampIdentity
    } else {
        StatsSettlement::Rebuild
    }
}

/// 一个 Ref0 的统计聚合结果（服务端 `GROUP BY` 直接产出，一个 Ref0 一行）。
#[derive(Deserialize)]
struct PeStatGroup {
    /// `record::id(id)` 下划线左半段，仍是字符串：投影里不做 `<int>` 转换，
    /// 好让形制不对的 id 在 Rust 侧报出原文，而不是在 SurrealQL 里静默成 NONE。
    ref0: String,
    count: usize,
    /// 投影里已经 `sesno ?? 0`，正常永远是数值；留 `Option` 只是不让一个
    /// 意外的 NONE 把整次重建打成失败。
    max_sesno: Option<i32>,
    max_ref1: Option<u64>,
}

#[cfg(test)]
mod pe_stat_row_tests {
    use super::{PeStatGroup, preserve_unparsed_pe_metadata};
    use aios_core::db::{DbBasicData, EleDataEntry};
    use aios_core::pdms_types::RefU64;
    use dashmap::DashMap;
    use std::collections::BTreeMap;

    /// 会话号缺失是历史 pe 行的常态（早于逐元素会话跟踪）。投影里的 `sesno ?? 0`
    /// 已经把它折平，但聚合列本身仍可能整列缺席（空组、旧引擎），解码不许因此失败。
    #[test]
    fn legacy_null_session_is_accepted() {
        let group: PeStatGroup = serde_json::from_value(serde_json::json!({
            "ref0": "24384",
            "count": 3,
            "max_sesno": null,
            "max_ref1": null,
        }))
        .unwrap();

        assert_eq!(group.ref0, "24384");
        assert_eq!(group.count, 3);
        assert_eq!(group.max_sesno.unwrap_or_default(), 0);
        assert_eq!(group.max_ref1.unwrap_or_default(), 0);
    }

    #[test]
    fn parse_failure_keeps_minimal_pe_metadata_for_owner_chain() {
        let refno = RefU64::from(0x0000_0002_0000_0003_u64);
        let owner = RefU64::from(0x0000_0001_0000_0009_u64);
        let mut bytes = vec![0_u8; 64];
        let pos = 8_usize;
        bytes[pos + 12..pos + 20].copy_from_slice(&owner.0.to_be_bytes());

        let refno_table_map = DashMap::new();
        refno_table_map.insert(
            refno,
            EleDataEntry {
                pos,
                noun_hash: aios_core::tool::db_tool::db1_hash("STRU") as i32,
            },
        );
        let db_basic = DbBasicData {
            bytes,
            refno_table_map,
            ..Default::default()
        };
        let parsed = DashMap::new();
        let sessions = BTreeMap::from([(17, 0_u32..1_u32)]);

        let preserved = preserve_unparsed_pe_metadata(&db_basic, &[refno], &sessions, &parsed);

        assert_eq!(preserved, vec![refno]);
        let attributes = parsed.get(&refno).expect("minimal metadata row");
        assert_eq!(attributes.get_type(), "STRU");
        assert_eq!(attributes.get_refno_or_default().refno(), refno);
        assert_eq!(
            attributes.get_refno_by_att_or_default("OWNER").refno(),
            owner
        );
        assert_eq!(attributes.sesno(), 17);
    }

    #[tokio::test]
    #[ignore = "manual live: requires the AMS 7324 fixture"]
    async fn live_7324_parse_failure_is_preserved_as_pe_metadata() {
        use std::path::PathBuf;
        use std::str::FromStr;
        use std::sync::Arc;

        let path = PathBuf::from(
            std::env::var("GEN_MODEL_OWNER_FIXTURE")
                .expect("GEN_MODEL_OWNER_FIXTURE points to ams7324_0001"),
        );
        let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
        let required_owner = RefU64::from_str("23708_48798").unwrap();
        let db_basic = Arc::new(
            parse_pdms_db::parse::parse_file_db_basic_data(&path, &file_name, "AvevaMarineSample")
                .unwrap(),
        );
        let parsed = parse_pdms_db::parse::parse_file_with_chunk(
            db_basic.clone(),
            &file_name,
            "AvevaMarineSample",
            &[required_owner],
            &BTreeMap::new(),
            true,
        )
        .await
        .unwrap();

        assert!(
            !parsed.total_attr_map.contains_key(&required_owner),
            "fixture must continue to exercise the full-decoder failure"
        );
        let preserved = preserve_unparsed_pe_metadata(
            &db_basic,
            &[required_owner],
            &BTreeMap::new(),
            &parsed.total_attr_map,
        );
        assert_eq!(preserved, vec![required_owner]);
        let metadata = parsed.total_attr_map.get(&required_owner).unwrap();
        assert_eq!(metadata.get_refno_or_default().refno(), required_owner);
        assert!(
            metadata
                .get_refno_by_att_or_default("OWNER")
                .refno()
                .is_valid()
        );
        assert_ne!(metadata.get_type(), "unset");
    }
}

/// 从 pe 全量重算一个 dbnum 的 `dbnum_info_table` 统计（DELETE + 按 ref0 重建）。
///
/// 事件只做增量维护：漏记（如事件曾被坏版本覆盖）造成的 count 缺口不会自愈，
/// 这里是唯一的纠偏入口。基线路径（`initialize_dbnum_baseline`）在统计不齐时
/// 自动调用；已有基线的库用 `rebuild_dbnum_stats` bin 手动触发。
///
/// **聚合必须留在服务端。** 旧写法是 `SELECT record::id(id) AS key, sesno FROM pe
/// WHERE dbnum = N`，把整个库的 pe 行拉回客户端再在 Rust 里分组。目录库的量级
/// 直接把 ws 连接打死：2026-08-18 现场 ams7351 有 3,345,853 行，语句吊了 9 分钟后
/// router 任务连同 channel 一起没了，报 `receiving from an empty and closed
/// channel`，把前面那趟 2.6 小时的全量解析整个作废（数据其实已经全部落库、统计
/// 也由事件维护到位，死在的是这次纯多余的回读）。改成 `GROUP BY ref0` 之后返回
/// 的行数等于 Ref0 个数（个位数），传输量与内存都与库的大小无关。
///
/// 代价说清楚：服务端逐行 `string::split` 不便宜，同一个 3.3M 行的库实测 861 秒。
/// 这条路径只在统计**真的**对不上时才该走——常态收口见
/// [`settle_dbnum_info_after_total_sync`]。
pub async fn rebuild_dbnum_info_from_pe(
    dbnum: u32,
    file_name: &str,
    db_type: &str,
) -> anyhow::Result<usize> {
    let mut response = SUL_DB
        .query(pe_stat_groups_sql(dbnum))
        .await
        .map_err(|error| anyhow::anyhow!("read PE stats dbnum={dbnum} failed: {error}"))?
        .check()
        .map_err(|error| {
            anyhow::anyhow!("read PE stats dbnum={dbnum} statement failed: {error}")
        })?;
    let groups: Vec<PeStatGroup> = response
        .take(0)
        .map_err(|error| anyhow::anyhow!("decode PE stats dbnum={dbnum} failed: {error}"))?;

    let mut by_ref0: BTreeMap<u64, (usize, i32, u64)> = BTreeMap::new();
    for group in groups {
        let ref0 = group.ref0.parse::<u64>().map_err(|error| {
            anyhow::anyhow!("invalid PE record id Ref0 {}: {error}", group.ref0)
        })?;
        by_ref0.insert(
            ref0,
            (
                group.count,
                group.max_sesno.unwrap_or_default(),
                group.max_ref1.unwrap_or_default(),
            ),
        );
    }

    execute_surreal_checked(
        &format!("DELETE dbnum_info_table WHERE dbnum = {dbnum};"),
        &format!("reset dbnum_info_table dbnum={dbnum}"),
    )
    .await?;
    let file_name = file_name.replace('\'', "\\'");
    let db_type = db_type.replace('\'', "\\'");
    let mut counted = 0_usize;
    for (ref0, (count, max_sesno, max_ref1)) in by_ref0 {
        execute_surreal_checked(
            &format!(
                "UPSERT dbnum_info_table:{ref0} SET dbnum = {dbnum}, count = {count}, \
                 sesno = {max_sesno}, max_ref1 = {max_ref1}, \
                 file_name = '{file_name}', db_type = '{db_type}';"
            ),
            &format!("rebuild dbnum_info_table dbnum={dbnum} ref0={ref0}"),
        )
        .await?;
        counted += count;
    }
    Ok(counted)
}

/// `dbnum` 的两侧计数：pe 实际行数、`dbnum_info_table` 的统计行数与 count 之和。
///
/// 两条都是索引支撑的服务端聚合（1112 实测各 20ms 级），跟按 Ref0 分组那条
/// 逐行 `string::split` 的语句不是一个量级——所以「要不要重建」值得先问一次。
async fn dbnum_stat_totals(dbnum: u32) -> anyhow::Result<(usize, usize, usize)> {
    #[derive(Deserialize)]
    struct PeCountRow {
        count: usize,
    }
    #[derive(Deserialize)]
    struct InfoTotalRow {
        row_count: usize,
        total: Option<usize>,
    }

    let mut response = SUL_DB
        .query(format!(
            "SELECT count() AS count FROM pe WHERE dbnum = {dbnum} GROUP ALL; \
             SELECT count() AS row_count, math::sum(count) AS total \
             FROM dbnum_info_table WHERE dbnum = {dbnum} GROUP ALL;"
        ))
        .await
        .map_err(|error| anyhow::anyhow!("read stat totals dbnum={dbnum} failed: {error}"))?
        .check()
        .map_err(|error| {
            anyhow::anyhow!("read stat totals dbnum={dbnum} statement failed: {error}")
        })?;
    let pe_rows: Vec<PeCountRow> = response
        .take(0)
        .map_err(|error| anyhow::anyhow!("decode PE count dbnum={dbnum} failed: {error}"))?;
    let info_rows: Vec<InfoTotalRow> = response
        .take(1)
        .map_err(|error| anyhow::anyhow!("decode stats total dbnum={dbnum} failed: {error}"))?;
    let pe_count = pe_rows.first().map(|row| row.count).unwrap_or_default();
    let (info_rows, info_count) = info_rows
        .first()
        .map(|row| (row.row_count, row.total.unwrap_or_default()))
        .unwrap_or_default();
    Ok((pe_count, info_rows, info_count))
}

/// total sync 结尾的统计收口：事件在场时统计已经是对的，不必再整表重算一遍。
///
/// `sync_total_async_threaded` 过去无条件调 [`rebuild_dbnum_info_from_pe`]。但
/// 这条路径并不摘事件（摘事件的是 `sync_pdms` / `sync_sys_only`），统计一路由
/// CREATE 分支维护到位——2026-08-18 现场 ams7351 的 pe 与统计都是 3,345,853，
/// 分毫不差，那次重算从头到尾没有纠正任何东西，只贡献了一次把连接打死的全表回读。
///
/// 于是先用两条便宜的计数问一句「对不对得上」：对得上就只补事件写不出的身份字段
/// （`file_name` / `db_type`，`rebuild_dbnum_stats` bin 拿它做身份兜底），对不上
/// 才付全量重算的代价。
async fn settle_dbnum_info_after_total_sync(
    dbnum: u32,
    file_name: &str,
    db_type: &str,
) -> anyhow::Result<()> {
    let (pe_count, info_rows, info_count) = dbnum_stat_totals(dbnum).await?;
    if classify_stats_settlement(pe_count, info_rows, info_count) == StatsSettlement::StampIdentity
    {
        execute_surreal_checked(
            &stamp_dbnum_info_identity_sql(dbnum, file_name, db_type),
            &format!("stamp dbnum_info_table identity dbnum={dbnum}"),
        )
        .await?;
        println!(
            "dbnum={dbnum} 统计与 pe 一致（{pe_count} 行 / {info_rows} 个 Ref0），\
             跳过全量重算，只补身份字段"
        );
        return Ok(());
    }
    println!(
        "dbnum={dbnum} 统计与 pe 不一致（pe={pe_count} 统计={info_count} 行数={info_rows}），\
         从 pe 全量重算"
    );
    rebuild_dbnum_info_from_pe(dbnum, file_name, db_type).await?;
    Ok(())
}

#[cfg(feature = "sql")]
pub trait MySqlMethods {
    fn add_to_args(&self, args: &mut sqlx::mysql::MySqlArguments);

    fn get_query(count: usize) -> anyhow::Result<String>;

    fn name() -> String;
}

/// 初始化project database
#[cfg(feature = "sql")]
pub async fn create_project_database(project: &str, url: &str) -> anyhow::Result<()> {
    let pool = MySqlPool::connect(url).await.unwrap();
    sqlx::query(&format!(
        "CREATE DATABASE IF NOT EXISTS {project} DEFAULT CHARSET UTF8"
    ))
    .execute(&pool)
    .await?;
    Ok(())
}

/// 初始化 info 库和表
#[cfg(feature = "sql")]
pub async fn create_info_database(aios_mgr: &AiosDBMgr) -> anyhow::Result<()> {
    let pool = AiosDBMgr::get_global_pool().await?;
    let project_name = aios_mgr.db_option.project_name.clone();
    pool.execute(
        format!(
            "CREATE DATABASE IF NOT EXISTS {PDMS_INFO_DB}_{};",
            project_name
        )
        .as_str(),
    )
    .await?;

    //todo 改成一对多的实现
    let mut sql = String::new();
    sql.push_str(&format!(r#"CREATE TABLE IF NOT EXISTS {} ("#, {
        PDMS_REFNO_INFOS_TABLE
    }));
    // sql.push_str(&format!(r#"{} BIGINT NOT NULL PRIMARY KEY ,"#, "REF0"));
    sql.push_str(&format!(r#"{} BIGINT UNSIGNED PRIMARY KEY ,"#, "ID"));
    sql.push_str(&format!(r#"{} BIGINT NOT NULL ,"#, "REF0"));
    //允许有多个project的存在
    sql.push_str(&format!(r#"{} VARCHAR(100)"#, "PROJECT"));

    sql.push_str(");");
    let result = pool.execute(sql.as_str()).await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(e);
            dbg!(sql.as_str());
        }
    }

    let result = pool
        .execute(gen_create_dbno_infos_tables_sql().as_str())
        .await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(&e);
        }
    }
    let result = pool
        .execute(gen_create_version_info_table_sql(&project_name).as_str())
        .await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(&e);
        }
    }
    let pools = aios_mgr.get_project_pools().await?;
    for (_, pool) in pools {
        let result = pool.execute(gen_create_element_tables_sql().as_str()).await;
        match result {
            Ok(_) => {}
            Err(e) => {
                dbg!(&e);
            }
        }
        let result = pool.execute(gen_create_project_mdb_sql().as_str()).await;
        match result {
            Ok(_) => {}
            Err(e) => {
                dbg!(&e);
            }
        }
    }

    Ok(())
}

/// 初始化同步pdms数据到数据
/// , progress_sender: Sender<i32>
fn full_sync_catalogue_files(
    db_option: &DbOption,
    db_type: &str,
) -> anyhow::Result<HashMap<String, Vec<String>>> {
    let mut candidates = Vec::new();
    for project in &db_option.included_projects {
        let project_dir =
            crate::data_interface::project_paths::resolve_project_root(db_option, project)
                .ok_or_else(|| anyhow::anyhow!("无法解析项目目录: {project}"))?;
        for path in collect_project_db_files(&project_dir, None)? {
            let mut header = [0u8; 60];
            std::fs::File::open(&path)
                .and_then(|mut file| file.read_exact(&mut header))
                .with_context(|| format!("读取候选数据库头失败: {}", path.display()))?;
            let info = parse_file_basic_info(&header);
            if info.db_type.eq_ignore_ascii_case(db_type) {
                candidates.push(
                    crate::data_interface::initialization_phase::CatalogueCandidate {
                        project: project.clone(),
                        dbnum: info.db_no,
                        path,
                    },
                );
            }
        }
    }
    let selection = crate::data_interface::initialization_phase::select_catalogue_candidates(
        candidates,
        &db_option.included_projects,
        &crate::options::catalogue_project_priority(),
    );
    if !selection.blockers.is_empty() {
        anyhow::bail!(
            "{db_type} 全量清单身份阻断: {}",
            selection.blockers.join("; ")
        );
    }
    let selected_project_by_dbnum = selection
        .selected
        .iter()
        .map(|candidate| (candidate.dbnum, candidate.project.clone()))
        .collect::<HashMap<_, _>>();
    for candidate in selection.shadowed {
        println!(
            "[full-sync] {db_type} dbnum={} 的 {} 被 {} 遮蔽: {}",
            candidate.dbnum,
            candidate.project,
            selected_project_by_dbnum
                .get(&candidate.dbnum)
                .map(String::as_str)
                .unwrap_or("<unknown>"),
            candidate.path.display()
        );
    }
    let mut by_project: HashMap<String, Vec<String>> = HashMap::new();
    for candidate in selection.selected {
        let file_name = candidate
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                anyhow::anyhow!("数据库文件名不是 UTF-8: {}", candidate.path.display())
            })?;
        by_project
            .entry(candidate.project)
            .or_default()
            .push(file_name.to_string());
    }
    Ok(by_project)
}

pub async fn sync_pdms(db_option: &DbOption) -> anyhow::Result<()> {
    if db_option.included_projects.is_empty() {
        return Err(anyhow::anyhow!("没有包含的项目"));
    }
    // 开始同步pdms/E3D项目的数据
    println!("开始同步pdms/E3D: {} 的数据", &db_option.project_name);
    // 计时器开始
    let mut time = tokio::time::Instant::now();

    // 解析前移除EVENT，防止大量的event触发
    println!("正在移除dbnum_event以提高解析性能...");
    let remove_event_sql = "REMOVE EVENT update_dbnum_event ON pe;";
    match SUL_DB.query(remove_event_sql).await {
        Ok(_) => println!("成功移除update_dbnum_event"),
        Err(e) => println!("移除update_dbnum_event失败（可能不存在）: {:?}", e),
    }

    // 获取默认的数据库连接字符串
    if db_option.sync_tidb.unwrap_or(false) {
        #[cfg(feature = "sql")]
        {
            let aios_mgr = AiosDBMgr::init_from_db_option().await?;
            create_info_database(&aios_mgr).await?;
        }
    }

    //只有重新同步时，才需要定义index
    let enable_index = db_option.total_sync || db_option.enable_index.unwrap_or(true);
    if enable_index {
        retry_surreal_write_operation("define owner index", aios_core::define_owner_index).await?;
        retry_surreal_write_operation("define geometry index", aios_core::create_geom_index)
            .await?;
        // aios_core::define_fullname_index().await.unwrap();
        retry_surreal_write_operation("define pe index", aios_core::define_pe_index).await?;
    }
    if db_option.is_sync_history() {
        retry_surreal_write_operation("define session index", aios_core::define_ses_index).await?;
    }

    let mut dbno_set = Arc::new(DashSet::new());
    let mut create_tables_elapse = 0;
    // 执行多线程解析
    dbg!("执行多线程解析");
    let proj_progress_chunk = 80 / db_option.included_projects.len();
    // ADR-025: full synchronization is global-by-phase, never project-by-project.
    let debug_refnos: Vec<RefU64> = db_option
        .debug_root_refnos
        .as_ref()
        .map(|roots| {
            roots
                .iter()
                .map(|root| RefU64::from_str(root).unwrap())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let _is_debug = !debug_refnos.is_empty();
    let main_project = &db_option.included_projects[0];
    let dict_files = full_sync_catalogue_files(db_option, "DICT")?;
    let cata_files = if db_option.only_sync_sys {
        HashMap::new()
    } else {
        full_sync_catalogue_files(db_option, "CATA")?
    };

    {
        sync_total_async_threaded(
            db_option,
            main_project,
            dbno_set.clone(),
            &["SYST", "GLB", "GLOB"],
            proj_progress_chunk,
        )
        .await
        .with_context(|| format!("同步 {main_project} 的 SYST/GLB/GLOB 数据失败"))?;

        // DICT belongs to Meta, including DICT supplied by dependency projects.
        // Process projects in explicit priority order so the shared naked-dbnum
        // guard deterministically keeps the authoritative file.
        let priority = crate::options::catalogue_project_priority();
        let mut ordered_projects = Vec::with_capacity(db_option.included_projects.len());
        let mut seen = std::collections::HashSet::new();
        for project in &priority {
            let Some(configured) = db_option
                .included_projects
                .iter()
                .find(|candidate| candidate.eq_ignore_ascii_case(project))
            else {
                anyhow::bail!("catalogue_project_priority 含未知项目 {project}");
            };
            if !seen.insert(configured.to_ascii_lowercase()) {
                anyhow::bail!("catalogue_project_priority 重复项目 {project}");
            }
            ordered_projects.push(configured.clone());
        }
        for project in &db_option.included_projects {
            if seen.insert(project.to_ascii_lowercase()) {
                ordered_projects.push(project.clone());
            }
        }

        for project in &ordered_projects {
            let mut phase_option = db_option.clone();
            phase_option.included_db_files =
                Some(dict_files.get(project).cloned().unwrap_or_default());
            sync_total_async_threaded(
                &phase_option,
                project,
                dbno_set.clone(),
                &["DICT"],
                proj_progress_chunk,
            )
            .await
            .with_context(|| format!("同步 {project} 的 DICT 数据失败"))?;
        }

        if db_option.only_sync_sys {
            println!("全局 Meta 阶段完成（SYST/GLB/GLOB + included DICT）");
        }
    }

    if !db_option.only_sync_sys {
        let priority = crate::options::catalogue_project_priority();
        let mut ordered_projects = Vec::with_capacity(db_option.included_projects.len());
        for project in &priority {
            let configured = db_option
                .included_projects
                .iter()
                .find(|candidate| candidate.eq_ignore_ascii_case(project))
                .ok_or_else(|| {
                    anyhow::anyhow!("catalogue_project_priority 含未知项目 {project}")
                })?;
            if ordered_projects
                .iter()
                .any(|seen: &String| seen.eq_ignore_ascii_case(configured))
            {
                anyhow::bail!("catalogue_project_priority 重复项目 {project}");
            }
            ordered_projects.push(configured.clone());
        }
        for project in &db_option.included_projects {
            if !ordered_projects
                .iter()
                .any(|seen| seen.eq_ignore_ascii_case(project))
            {
                ordered_projects.push(project.clone());
            }
        }

        // Global Catalogue barrier. Fail-fast before any Design call begins.
        for project in &ordered_projects {
            let mut phase_option = db_option.clone();
            phase_option.included_db_files =
                Some(cata_files.get(project).cloned().unwrap_or_default());
            sync_total_async_threaded(
                &phase_option,
                project,
                dbno_set.clone(),
                &["CATA"],
                proj_progress_chunk,
            )
            .await
            .with_context(|| format!("同步 {project} 的 CATA 数据失败"))?;
        }

        // Global Design barrier starts only after every selected CATA settles.
        for project in &db_option.included_projects {
            sync_total_async_threaded(
                db_option,
                project,
                dbno_set.clone(),
                &["DESI"],
                proj_progress_chunk,
            )
            .await
            .with_context(|| format!("同步 {project} 的 DESI 数据失败"))?;
        }
    }

    // 解析完成后重新定义EVENT
    println!("正在重新定义dbnum_event...");
    match define_dbnum_event().await {
        Ok(_) => println!("成功重新定义update_dbnum_event"),
        Err(e) => println!("重新定义update_dbnum_event失败: {:?}", e),
    }

    // 输出创建表所花费的时间
    println!("创建表花费时间: {} ms", create_tables_elapse);
    // 输出初始化数据库所花费的时间
    println!(
        "初始化数据库时间: {} ms",
        time.elapsed().as_millis() - create_tables_elapse
    );

    Ok(())
}

/// `pe` 表统计维护事件的**唯一**定义（回归测试与生产装载共用这一份文本）。
pub(crate) fn dbnum_event_sql() -> &'static str {
    r#"
    DEFINE EVENT OVERWRITE update_dbnum_event ON pe WHEN $event = "CREATE" OR $event = "UPDATE" OR $event = "DELETE" THEN {
            -- 获取当前记录的 dbnum
            LET $dbnum = $value.dbnum;
            LET $id = record::id($value.id);
            let $id_parts = string::split($id, "_");
            let $ref_0 = <int>array::at($id_parts, 0);
            let $ref_1 = <int>array::at($id_parts, 1);
            let $is_delete = $value.deleted and $event = "UPDATE";
            -- NONE 免疫（2026-08-06 审计）：补链创建的 WORL `/*` 带 sesno=0，旧写法
            -- `IF $after.sesno > $before.sesno?:0` 在 CREATE 时两边都不成立，取到
            -- $before.sesno = NONE，MERGE 后 info 行**没有 sesno 字段**，读侧
            -- math::max 直接炸（1112 / 7999 实测）。max 套 ?:0 后恒为数值。
            let $max_sesno = math::max([$after.sesno?:0, $before.sesno?:0]);
            -- 根据事件类型处理  type::thing("dbnum_info_table", $ref_0)
            -- 页内水位只升不降：sesno=0 的伪元素后到时不得把已见过的会话号抹小。
            IF $event = "CREATE"   {
                UPSERT type::thing('dbnum_info_table', $ref_0) MERGE {
                    dbnum: $dbnum,
                    count: count?:0 + 1,
                    sesno: math::max([sesno?:0, $max_sesno]),
                    max_ref1: $ref_1,
                    updated_at: time::now()
                };
            } ELSE IF $event = "DELETE" OR $is_delete  {
                -- UPDATE 而不是 UPSERT（2026-08-18）：统计行缺席是**常态**——
                -- `sync_pdms` / `sync_sys_only` 为了性能先 REMOVE EVENT 再写 pe，
                -- 那批行天生没有统计行（实测 ams5100 有 236 条 pe、零条统计行）。
                -- UPSERT 在缺行时走创建路径，`WHERE count > 0` 拦不住它，
                -- `NONE - 1` 当场把整条 DELETE 语句打死：清库重建与首次按需初始化
                -- 都过不去（2026-08-18 现场：dbnum 5100 批次 failed，报
                -- "Cannot perform subtraction with 'NONE' and '1'"）。而且这条
                -- MERGE 不写 dbnum，创建出来的行连 `DELETE ... WHERE dbnum = N`
                -- 都清不掉。缺行意味着这个 Ref0 的统计本就没在维护，交给
                -- `rebuild_dbnum_info_from_pe` 重算，不在这里凭空造一行。
                UPDATE type::thing('dbnum_info_table', $ref_0) MERGE {
                    count: count?:0 - 1,
                    sesno: math::max([sesno?:0, $max_sesno]),
                    max_ref1: $ref_1,
                    updated_at: time::now()
                }
                WHERE count > 0;
            }  ELSE IF $event = "UPDATE" {
                UPSERT type::thing('dbnum_info_table', $ref_0) MERGE {
                    sesno: math::max([sesno?:0, $max_sesno]),
                    updated_at: time::now()
                };
            };
        };
    "#
}

pub async fn define_dbnum_event() -> anyhow::Result<()> {
    SUL_DB.query(dbnum_event_sql()).await?;

    // 定义后立即读回自证。这个事件有过不兼容的同名实现（rs-core 里对 string 形态
    // pe id 用 array::at 解析的版本，$ref_0 恒为 NONE，统计维护整体静默断供），
    // 谁最后启动谁 OVERWRITE。多服务混跑同一个库时，这里的告警是唯一能在启动
    // 日志里看见「事件被换成坏版」的地方。
    match verify_dbnum_event_definition().await {
        Ok(true) => {}
        Ok(false) => {
            eprintln!(
                "update_dbnum_event 事件体校验失败：库里生效的定义不含 string::split 指纹，\
                 dbnum_info_table 统计维护可能已静默失效（多半是别的进程用旧实现覆盖了它）"
            );
        }
        Err(error) => {
            eprintln!("update_dbnum_event 事件体读回失败（不阻断启动）: {error:#}");
        }
    }
    Ok(())
}

/// 读回 pe 表上 `update_dbnum_event` 的实际定义，校验是好版（string::split 解析
/// string 形态 id）。返回 `false` 表示事件缺失或被不兼容实现覆盖。
pub async fn verify_dbnum_event_definition() -> anyhow::Result<bool> {
    let mut response = SUL_DB
        .query("INFO FOR TABLE pe;")
        .await
        .map_err(|e| anyhow::anyhow!("读取 pe 表定义失败: {e}"))?
        .check()
        .map_err(|e| anyhow::anyhow!("读取 pe 表定义语句失败: {e}"))?;
    let info: Option<serde_json::Value> = response
        .take(0)
        .map_err(|e| anyhow::anyhow!("解码 pe 表定义失败: {e}"))?;
    let body = info
        .as_ref()
        .and_then(|v| v.get("events"))
        .and_then(|events| events.get("update_dbnum_event"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    Ok(body.contains("string::split"))
}

#[cfg(feature = "sql")]
pub async fn execute_sql(conn: &Pool<MySql>, sql: &str) -> bool {
    return match conn.execute(sql).await {
        Ok(_) => true,
        Err(e) => {
            match &e {
                Error::Database(error) => {
                    //index already exist
                    if error.code() == Some(Cow::from("42000")) {
                    } else {
                        dbg!(sql);
                    }
                }
                _ => {
                    dbg!(&e);
                }
            }
            false
        }
    };
}

pub async fn check_and_clear_db(db_no: u32) -> anyhow::Result<()> {
    let result = crate::data_interface::fast_delete::delete_dbnum_fast(db_no).await?;
    if result.pe_rows > 0 {
        println!(
            "dbnum={} 快速删除完成：PE={}，Ref0={:?}，noun 表={}，区间语句={}，耗时={}ms",
            result.dbnum,
            result.pe_rows,
            result.ref0s,
            result.noun_tables,
            result.range_statements,
            result.elapsed_ms
        );
    }
    Ok(())
}

//分成两部分，一部分先保存UDA 和 SYS 这些数据
///多线程同步数据，包括增量同步
pub async fn sync_total_async_threaded(
    db_option: &DbOption,
    project: &str,
    cur_dbno_set: Arc<DashSet<u32>>,
    db_types: &[&str],
    // progress_sender: Sender<i32>,
    proj_progress_chunk: usize,
) -> anyhow::Result<HashMap<u32, usize>> {
    println!("开始解析 {project} 的 {:?}", db_types);
    let db_option_arc = Arc::new(db_option.clone()); // 创建一个Arc对象，表示数据库选项
    // 与监控目录同一套解析：只认 included_projects 中的文件夹名，并固定落在
    // project_path 下，避免初始化绕过当期扫描范围。
    let project_dir =
        crate::data_interface::project_paths::resolve_project_root(db_option, project)
            .ok_or_else(|| anyhow::anyhow!("无法解析项目目录: {project}"))?; // 创建一个Path对象，表示项目目录的路径
    dbg!(&project_dir);

    if !Path::new(&project_dir).exists() {
        dbg!("项目文件夹指定不正确");
        // 如果项目目录不存在，则抛出错误
        return Err(anyhow::anyhow!("项目文件夹指定不正确"));
    }
    let children_files = collect_project_db_files(
        Path::new(&project_dir),
        db_option.included_db_files.as_deref(),
    )?;
    // println!("需要处理的文件: {:?}", &children_files);
    // dbg!(children_files.len());
    // 先解析一遍uda
    // 正式解析
    #[cfg(feature = "sql")]
    let mgr = AiosDBMgr::init_from_db_option().await?;
    let project = Arc::new(project.to_string()); // 创建一个Arc对象，表示项目名称
    let mut is_replace = db_option_arc.replace_dbs; // 是否替换数据库的数据
    let replace_types = db_option_arc.replace_types.clone(); // 获取替换的类型列表
    let b_replace_types = replace_types.is_some(); // 是否存在替换的类型列表
    // 是否保存到tidb
    let b_save_mysql = db_option_arc.sync_tidb.unwrap_or(false);
    if b_replace_types {
        is_replace = true;
    }
    let chunk_size = db_option_arc.sync_chunk_size.unwrap_or(10_0000) as usize;
    // let sync_tidb = db_option_arc.sync_tidb.unwrap_or(false);
    #[cfg(feature = "sql")]
    let pool = mgr.get_project_pools().await.unwrap_or_default();

    // Apply backpressure while parsing large single-file baselines. An
    // unbounded queue can retain every pending Surreal INSERT payload and
    // exhaust the interactive viewer process before the workers catch up.
    let (sender, receiver) = flume::bounded(BASELINE_QUEUE_CAPACITY);

    let mut insert_handles = FuturesUnordered::new();
    // SurrealDB 2.1 uses optimistic transactions. Concurrent multi-row writes
    // to disjoint PE ids still contend on shared table/index state and a real
    // 7997 baseline exhausted the conflict retry budget. Parsing remains
    // parallel; persistence is deliberately single-writer and still retains
    // checked conflict retries for interference from other processes.
    for _ in 0..BASELINE_WRITE_WORKERS {
        let receiver: flume::Receiver<SenderJsonsData> = receiver.clone();
        #[cfg(feature = "sql")]
        let pools_clone = pool.clone();

        let insert_handle = tokio::task::spawn(async move {
            // Must remain below the bounded channel capacity (100). With one
            // correctness-first writer, chunks(200) waits for 200 messages
            // while the producer blocks after 100: a deterministic deadlock.
            let mut record_stream = receiver.into_stream().chunks(BASELINE_WRITE_WINDOW);
            let mut error_count = 0usize;
            let mut error_samples = Vec::new();
            // let mut cnt = 0;
            while let Some(stream) = record_stream.next().await {
                // while let Ok(data) = receiver.recv_async().await {
                for data in stream {
                    match data {
                        SenderJsonsData::PEJson(pes) => {
                            if !pes.is_empty() {
                                let sql = format!("INSERT IGNORE INTO pe [{}]", pes.join(","));
                                if let Err(error) =
                                    execute_surreal_checked(&sql, "insert PE batch").await
                                {
                                    record_write_error(
                                        &mut error_count,
                                        &mut error_samples,
                                        "PE",
                                        error,
                                    );
                                }
                            }
                        }
                        SenderJsonsData::PERelateJson(relates) => {
                            if !relates.is_empty() {
                                // IGNORE 与 pe 同语义：边 id 显式（`pe_owner:[owner, i]`），
                                // 父层补缺重放叶子已写过的边时必须幂等，否则重复 id 报错
                                // 会把整次补缺同步打成失败。
                                let sql = format!(
                                    "INSERT RELATION IGNORE INTO pe_owner [{}]",
                                    relates.join(",")
                                );
                                if let Err(error) =
                                    execute_surreal_checked(&sql, "insert pe_owner batch").await
                                {
                                    record_write_error(
                                        &mut error_count,
                                        &mut error_samples,
                                        "pe_owner",
                                        error,
                                    );
                                }
                            }
                        }
                        SenderJsonsData::AttJson((table, atts)) => {
                            if !atts.is_empty() {
                                let sql =
                                    format!("INSERT IGNORE INTO {} [{}]", table, atts.join(","));
                                if let Err(error) =
                                    execute_surreal_checked(&sql, "insert attribute batch").await
                                {
                                    record_write_error(
                                        &mut error_count,
                                        &mut error_samples,
                                        &format!("attribute table {table}"),
                                        error,
                                    );
                                }
                            }
                        }
                        #[cfg(feature = "sql")]
                        SenderJsonsData::MysqlSql((project, sql)) => {
                            let Some(pool) = pools_clone.get(&project) else {
                                continue;
                            };
                            let mut conn = pool.acquire().await.expect("get pool failed");
                            match conn.execute(sql.as_str()).await {
                                Ok(_) => {}
                                Err(e) => {
                                    dbg!(e.to_string());
                                    dbg!(&sql);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            (error_count, error_samples)
        });
        insert_handles.push(insert_handle);
    }

    let db_types_clone = db_types
        .into_iter()
        .map(|&x| x.to_string())
        .collect::<Vec<_>>();
    let is_parse_sys = db_types_clone.contains(&"SYST".to_string());
    let is_save_db = db_option.is_save_db();
    let is_total_sync = db_option.total_sync;
    let sender_clone = sender.clone();
    let parsed_db_infos = Arc::new(DashMap::<u32, (String, String, usize)>::new());
    let parsed_db_infos_for_parser = parsed_db_infos.clone();
    let failed_baseline_dbnums = Arc::new(DashSet::<u32>::new());
    let failed_baseline_dbnums_for_parser = failed_baseline_dbnums.clone();
    let scheduled_baseline_dbnums = Arc::new(DashSet::<u32>::new());
    let scheduled_baseline_dbnums_for_parser = scheduled_baseline_dbnums.clone();
    let children_files_len = children_files.len();
    let db_file_progress_chunk = (proj_progress_chunk as f32 / children_files_len as f32) as usize;
    // let progress_sender_clone = progress_sender.clone();
    let parser_outcome = tokio::spawn(async move {
        //todo 按照文件大小排序，只有小于多少的能开启多线程，模型一大就不合适了
        // let mut db_info_sql = vec![];
        for path in children_files {
            let file_name = path.file_name().unwrap().to_str().unwrap().to_string(); // 获取文件名
            if file_name.contains(".") {
                continue;
            }
            let dbno_set = cur_dbno_set.clone();
            let mut time = Instant::now();
            // dbg!(&file_name);
            // gen-model-9 / ADR-007：SYS 元数据(SYST/DICT/GLB/GLOB)必须从其专属文件(amssys 等)解析，
            // 不应受 included_db_files 过滤——它列的是 DESI/CATA 数据库文件，从不含 SYS 文件。
            // 旧条件 `is_parse_sys && is_total_sync` 使得 only_sync_sys(非 total_sync) 下 SYS 文件被
            // included_db_files 过滤掉、静默跳过 → 设计 MDB/CURD/DB 建不起来。改为 is_parse_sys 即可：
            // SYS 同步始终遍历全部文件，再由下方 db_type 过滤只留 SYS 文件。DESI/CATA 同步(is_parse_sys=false)
            // 行为不变、仍受 included_db_files 约束。
            if is_parse_sys
                || db_option_arc.included_db_files.is_none()
                || db_option_arc
                    .included_db_files
                    .as_ref()
                    .unwrap()
                    .contains(&file_name)
            {
                if !is_total_sync {
                    // progress_sender_clone.send(db_file_progress_chunk).await.unwrap();
                }
                // dbg!(&file_name);
                let mut baseline_snapshot =
                    match pdms_io::snapshot::DabaconSnapshot::open(project.as_str(), &path) {
                        Ok(snapshot) => snapshot,
                        Err(error) => {
                            log::error!(
                                "打开基线 dabacon 快照失败 {}: {error:#}",
                                path.display()
                            );
                            continue;
                        }
                    };
                let db_type = baseline_snapshot.token().db_type().to_owned();
                let db_no = baseline_snapshot.token().dbnum() as u32;
                //如果不是全部解析，需要检查类型，全部解析一定要解析syst等配置文件数据库
                if !db_types_clone.contains(&db_type) {
                    continue;
                }
                //需要检查pe里是否有这个dbno，如果有，则需要改成使用upsert
                if is_replace {
                    check_and_clear_db(db_no).await.unwrap();
                }
                //保证不重复加载相同dbno的数据
                if dbno_set.contains(&db_no) {
                    continue;
                }
                // dbg!(db_no);
                dbno_set.insert(db_no);
                // 如果需要解析的文件列表为空或包含当前文件名，则执行以下代码块
                println!("path={:?}", &file_name); // 打印文件路径
                let sesno = baseline_snapshot.token().target_sesno();
                if sesno == 0 {
                    continue;
                }
                let ses_range_map = baseline_snapshot.session_ranges();

                let project_name = project.as_str().to_string(); // 获取项目名称的字符串
                // 解析失败绝不能退化成“空库”：`unwrap_or_default()` 会让本文件以 0 元素
                // 计入 parsed_db_infos，基线层据此认定合法空库并推进 applied_sesno，
                // 于是整个 dbnum 被静默跳过。跳过本文件、不登记结果，让基线层以
                // “解析未返回目标文件结果”显式失败。
                let mut db_basic = match baseline_snapshot.read_full_basic_data(
                    &file_name,
                    project_name.clone().as_str(),
                ) {
                    Ok(db_basic) => db_basic,
                    Err(error) => {
                        println!("解析 {file_name} 失败，跳过该文件: {error:#}");
                        continue;
                    }
                };
                // issue #10：解析的补链轮按记录自带的 owner 把「owner 成员表里没列出
                // 它」的父子边补回去，而那正是 E3D 表达删除的方式，于是已删子树被整棵
                // 复活、和重建出来的同名子树在库里并存。按元素自己的成员块把多挂的边
                // 摘掉，随之不可达的元素一并丢弃（判据与取舍见 `member_prune`）。
                {
                    let world = db_basic.world_refno;
                    let bytes = &db_basic.bytes;
                    let refno_table_map = &db_basic.refno_table_map;
                    let report = member_prune::prune_resurrected_members(
                        world,
                        &mut db_basic.children_map,
                        |refno| member_prune::authoritative_members(bytes, refno_table_map, refno),
                    );
                    if report.skipped_no_root_authority {
                        println!("{file_name}: 根成员表为空，跳过已删元素裁剪，保持解析原样");
                    } else if !report.is_empty() {
                        println!(
                            "{file_name}: 裁掉补链多挂的父子边 {} 条、随之不可达的元素 {} 个",
                            report.dropped_edges, report.dropped_elements
                        );
                    }
                }

                let all_refnos = db_basic
                    .children_map
                    .keys()
                    .filter(|refno| **refno != db_basic.world_refno)
                    .cloned()
                    .collect::<Vec<_>>();

                let db_basic = Arc::new(db_basic);
                if is_save_db {
                    scheduled_baseline_dbnums_for_parser.insert(db_no);
                    if let Err(error) = save_pe_relates(&db_basic, sender_clone.clone()).await {
                        log::error!(
                            "baseline relation dispatch failed: file={file_name} dbnum={db_no}: {error:#}"
                        );
                        failed_baseline_dbnums_for_parser.insert(db_no);
                        continue;
                    }
                }
                let debug_refnos: Vec<RefU64> = db_option_arc
                    .debug_root_refnos
                    .as_ref()
                    .map(|x| {
                        x.iter()
                            .map(|x| RefU64::from_str(x).unwrap())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                //debug 不保存数据，只复杂查看属性值
                let is_debug = !debug_refnos.is_empty();
                if is_debug {
                    let debug_refno = debug_refnos[0];
                    if let Some(children) = db_basic.children_map.get(&debug_refno) {
                        dbg!(children);
                    }
                }
                let debug_refnos = Arc::new(debug_refnos);
                //按照SITE划分？
                let mut total_cnt = 0;
                let mut chunk_failed = false;
                'chunks: for (chunk_index, chunk) in
                    all_refnos.chunks(chunk_size).enumerate()
                {
                    let db_option_clone = db_option_arc.clone();
                    let file_name_clone = file_name.clone();
                    let chunk_refnos = chunk.to_vec();
                    let project_name_clone = project_name.clone();
                    let db_basic_clone = db_basic.clone();
                    let debug_refnos = debug_refnos.clone();
                    let ses_range_map_clone = ses_range_map.clone();
                    let ignore_world_refno = true;
                    match parse_file_with_chunk(
                        db_basic_clone.clone(),
                        &file_name_clone,
                        project_name_clone.as_str(),
                        &chunk_refnos,
                        &ses_range_map_clone,
                        ignore_world_refno,
                    )
                    .await
                    {
                        Ok(PdmsDbData {
                            total_attr_map,
                            type_ele_map,
                            db_no,
                            ..
                        }) => {
                            let preserved = preserve_unparsed_pe_metadata(
                                &db_basic_clone,
                                &chunk_refnos,
                                &ses_range_map_clone,
                                &total_attr_map,
                            );
                            if preserved.is_empty() {
                                crate::data_interface::parse_error::note_attrs_success(
                                    &file_name_clone,
                                );
                            } else {
                                let samples = preserved
                                    .iter()
                                    .take(5)
                                    .map(ToString::to_string)
                                    .join(", ");
                                println!(
                                    "{file_name_clone}: {} 个元素完整属性解析失败，已保留 PE 拓扑元数据（样例: {samples}）",
                                    preserved.len()
                                );
                                // 只打一行的话，这批元素属性缺失事后无从查起：拓扑还在，
                                // 所以它们照常出现在树上、照常被引用，只是属性是残的。
                                crate::data_interface::parse_error::note_attrs_failure(
                                    &file_name_clone,
                                    preserved.len() as u64,
                                    &samples,
                                );
                            }
                            //类型暂时不多线程
                            let total_attr_map_arc = Arc::new(total_attr_map);
                            total_cnt += total_attr_map_arc.len();
                            //开始执行保存数据
                            println!("开始保存pe数量: {}", total_attr_map_arc.len());
                            if !is_debug && is_save_db {
                                if let Err(error) = save_pes(
                                    &db_basic_clone,
                                    &total_attr_map_arc,
                                    db_no as i32,
                                    &file_name_clone,
                                    &db_type,
                                    &db_option_clone,
                                    sender_clone.clone(),
                                )
                                .await
                                {
                                    log::error!(
                                        "baseline chunk PE dispatch failed: file={file_name_clone} dbnum={db_no} chunk={chunk_index}: {error:#}"
                                    );
                                    failed_baseline_dbnums_for_parser.insert(db_no);
                                    chunk_failed = true;
                                    break;
                                }
                            }
                            if b_save_mysql {
                                #[cfg(feature = "sql")]
                                save_pes_mysql(
                                    &db_basic_clone,
                                    &project_name,
                                    &total_attr_map_arc,
                                    &pool,
                                    &db_option_clone,
                                    db_no as i32,
                                )
                                .await;
                            }
                            for kv in type_ele_map.iter() {
                                let noun: i32 = *kv.key() as _;
                                let type_name = db1_dehash(noun as _);
                                let refnos = kv.value().iter().copied().collect::<Vec<_>>();
                                drop(kv);
                                if type_name.is_empty() {
                                    continue;
                                }
                                //UDA 还是要单独存，不然数据很容易混乱
                                for refnos in refnos.chunks(db_option_clone.att_chunk as _) {
                                    let mut json_vec = vec![];
                                    let mut uda_json_vec = vec![];
                                    for refno in refnos {
                                        let att = total_attr_map_arc.get(refno).unwrap();
                                        //调试时，只解析这个单独的refno
                                        if is_debug {
                                            if debug_refnos
                                                .contains(&att.get_refno_or_default().refno())
                                            {
                                                dbg!(att.value());
                                            } else {
                                                continue;
                                            }
                                        }
                                        if !is_save_db {
                                            continue;
                                        }
                                        let Some(json) = att.gen_sur_json() else {
                                            continue;
                                        };
                                        json_vec.push(json);
                                        let Some(json) = att.gen_sur_json_uda(&[]) else {
                                            continue;
                                        };
                                        uda_json_vec.push(normalize_sql_string(&json));
                                    }
                                    if is_save_db {
                                        if !json_vec.is_empty() {
                                            if let Err(error) = sender_clone
                                                .send_async(SenderJsonsData::AttJson((
                                                    type_name.clone(),
                                                    json_vec,
                                                )))
                                                .await
                                            {
                                                log::error!(
                                                    "baseline chunk attribute dispatch failed: file={file_name_clone} dbnum={db_no} chunk={chunk_index}: {error}"
                                                );
                                                failed_baseline_dbnums_for_parser.insert(db_no);
                                                chunk_failed = true;
                                                break 'chunks;
                                            }
                                        }

                                        if !uda_json_vec.is_empty() {
                                            // dbg!(&uda_json_vec);
                                            if let Err(error) = sender_clone
                                                .send_async(SenderJsonsData::AttJson((
                                                    "ATT_UDA".to_string(),
                                                    uda_json_vec,
                                                )))
                                                .await
                                            {
                                                log::error!(
                                                    "baseline chunk UDA dispatch failed: file={file_name_clone} dbnum={db_no} chunk={chunk_index}: {error}"
                                                );
                                                failed_baseline_dbnums_for_parser.insert(db_no);
                                                chunk_failed = true;
                                                break 'chunks;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            log::error!(
                                "baseline chunk failed: file={file_name_clone} dbnum={db_no} chunk={chunk_index}: {e:#}"
                            );
                            failed_baseline_dbnums_for_parser.insert(db_no);
                            chunk_failed = true;
                            break;
                        }
                    }
                }

                if chunk_failed {
                    println!(
                        "{file_name}: 基线 chunk 失败，停止该文件后续调度；等待写入收口后清理 dbnum={db_no}"
                    );
                    continue;
                }

                println!(
                    "解析任务完成, 耗时: {} s, 总数量: {}",
                    time.elapsed().as_secs_f32(),
                    total_cnt
                );
                parsed_db_infos_for_parser
                    .insert(db_no, (file_name.clone(), db_type.clone(), total_cnt));
            }
            //单个文件多线程
            // if !handles.is_empty() {
            //     dbg!(handles.len());
            //
            //     futures::future::join_all(take(&mut handles)).await;
            //
            // }
            //重新更新一下database info，有可能发生了更新
            // let db_info = get_default_pdms_db_info();
            // let _ = db_info.save(None);
        }

        //执行保存db_info sql
        // let db_info_sql = db_info_sql.join(";");
        // if !db_info_sql.is_empty() {
        //     SUL_DB.query(&db_info_sql).await.expect("save db_info failed");
        // }
        Ok::<(), anyhow::Error>(())
    })
    .await
    .map_err(|error| anyhow::anyhow!("baseline parser task failed: {error}"))
    .and_then(|inner| inner);
    // 解析任务是 spawn 出去的，攒下的属性解析失败在这里落库（见 `note_attrs_failure`）。
    // 排在 `?` 之前：解析失败恰恰是最需要留痕的那一次。
    if let Err(error) = crate::data_interface::parse_error::flush().await {
        log::warn!("{error:#}");
    }
    let pipeline_result = finish_write_pipeline(sender, insert_handles, parser_outcome).await;
    let mut cleanup_errors = Vec::new();
    let cleanup_dbnums = baseline_cleanup_targets(
        failed_baseline_dbnums.iter().map(|entry| *entry),
        scheduled_baseline_dbnums.iter().map(|entry| *entry),
        pipeline_result.is_err(),
    );
    for dbnum in cleanup_dbnums {
        if let Err(error) = crate::data_interface::fast_delete::wipe_dbnum_for_reinit(dbnum).await {
            cleanup_errors.push(format!("dbnum={dbnum}: {error:#}"));
        }
    }
    if !cleanup_errors.is_empty() {
        anyhow::bail!(
            "baseline chunk 失败后的清库未完成: {}",
            cleanup_errors.join("; ")
        );
    }
    pipeline_result?;
    if is_total_sync && is_save_db {
        for entry in parsed_db_infos.iter() {
            let dbnum = *entry.key();
            let (file_name, db_type, _) = entry.value();
            settle_dbnum_info_after_total_sync(dbnum, file_name, db_type).await?;
        }
    }
    // all_handles.push(parse_handle);
    // futures::future::join_all(take(&mut all_handles)).await;
    // futures::future::join_all(&mut [parse_handle]).await;
    Ok(parsed_db_infos
        .iter()
        .map(|entry| (*entry.key(), entry.value().2))
        .collect())
}

/// 给对应类型的参考号赋上 uda 默认值
fn set_uda_attr(
    type_ele_map: &DashMap<u32, HashSet<RefU64>>,
    total_attr_map: &DashMap<RefU64, WholeAttMap>,
    uda_map: &mut HashMap<i32, AttrMap>,
) -> anyhow::Result<()> {
    // if let Some(uda_refnos) = type_ele_map.get(&db1_hash("UDA")) {
    //     // 获取每个 uda 的 ELEL , DFLT , UDNA属性
    //     for uda_refno in uda_refnos.value() {
    //         let uda_att = total_attr_map.get(uda_refno);
    //         if uda_att.is_none() {
    //             continue;
    //         }
    //         let uda_att = uda_att.unwrap();
    //         let uda_implicit_att = &uda_att.implicit_attmap;
    //         let uda_explicit_att = &uda_att.explicit_attmap;

    //         let ukey = uda_implicit_att.get_i32("UKEY");
    //         if ukey.is_none() {
    //             continue;
    //         }
    //         let ukey = ukey.unwrap();
    //         // 若udna中没有值，则可能在显式属性的dyudna中
    //         let mut udna = uda_implicit_att.get_str("UDNA");
    //         if udna == Some("") {
    //             udna = uda_explicit_att.get_str("DYUDNA");
    //         }
    //         let elel = uda_explicit_att.get_i32_vec("ELEL");
    //         let default = uda_explicit_att.get_val("DFLT");
    //         if elel.is_none() || default.is_none() {
    //             continue;
    //         }
    //         // let udna = udna.unwrap();
    //         let elel = elel.unwrap();
    //         let default = default.unwrap();
    //         for noun in elel {
    //             uda_map
    //                 .entry(noun)
    //                 .or_insert_with(AttrMap::default)
    //                 .entry((ukey as u32))
    //                 .or_insert(default.clone());
    //         }
    //     }
    // }
    Ok(())
}

// pub fn gen_pdms_element_insert_sql(att: &WholeAttMap, name: &str, dbno: u32, order: usize, children_count: usize) -> String {
//     let attmap = &att.att_map();
//     let refno = attmap.get_refno().unwrap();
//     let type_name = attmap.get_type();
//     let owner = attmap.get_owner();
//
//     let mut sql = String::new();
//     sql.push_str(&format!(r#"({}, '{}', '{}', {},'{}' , {} , {} , {} ,0 ) ,"#,
//                           refno.0, refno.to_pdms_str(), type_name, owner.0, name, dbno, order, children_count));
//     sql
// }

#[tokio::test]
async fn test_threads() {
    let mut map = Arc::new(DashSet::new());
    let mut handles = vec![];
    for i in 0..10 {
        let map_clone = map.clone();
        let handle = tokio::spawn(async move {
            map_clone.insert(i);
        });
        handles.push(handle);
    }
    futures::future::join_all(take(&mut handles)).await;
    dbg!(&map.len());
    for v in Arc::try_unwrap(map).unwrap() {
        dbg!(v);
    }
}

#[test]
fn surreal_write_conflicts_are_retryable_but_syntax_errors_are_not() {
    assert!(is_retryable_surreal_write_error(
        "Failed to commit transaction due to a read or write conflict. This transaction can be retried"
    ));
    assert!(!is_retryable_surreal_write_error(
        "Parse error: unexpected token"
    ));
}

#[test]
fn baseline_writer_window_is_smaller_than_bounded_queue() {
    assert!(BASELINE_WRITE_WINDOW < BASELINE_QUEUE_CAPACITY);
    assert!(BASELINE_WRITE_WORKERS <= BASELINE_WRITE_WINDOW);
}

#[cfg(test)]
fn make_temp_dir(tag: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("gen-model-{tag}-{}-{unique}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn collect_project_db_files_skips_directories_and_prefers_extract_copy() {
    let root = make_temp_dir("dbfiles");
    let db_dir = root.join("TES000");
    std::fs::create_dir_all(&db_dir).unwrap();
    // 目录名不含 `.` 时会被当成待解析文件，Windows 上 File::open 打开目录返回
    // PermissionDenied，整个解析任务会 panic。
    std::fs::create_dir_all(db_dir.join("新建文件夹")).unwrap();
    for name in ["tes1002", "tes1002_0001", "tes1008", "tes1008.bak"] {
        std::fs::write(db_dir.join(name), b"stub").unwrap();
    }

    let mut files = collect_project_db_files(&root, None).unwrap();
    files.sort();
    let names = files
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    assert_eq!(names, vec!["tes1002_0001", "tes1008"]);
    assert!(files.iter().all(|path| path.is_file()));

    std::fs::remove_dir_all(&root).unwrap();
}

/// ADR-028 父层补缺回归：被叶子 shadow 的主库，若被 `included_db_files` 点名，
/// 必须回到解析清单。回退到「无条件丢 shadow」的旧写法时这里会红——那正是
/// 补缺同步静默空转（一个文件都不解析却返回 Ok）的根因。
#[test]
fn collect_project_db_files_keeps_explicitly_named_shadowed_master() {
    let root = make_temp_dir("dbfiles-parent");
    let db_dir = root.join("TES000");
    std::fs::create_dir_all(&db_dir).unwrap();
    for name in ["tes1002", "tes1002_0001"] {
        std::fs::write(db_dir.join(name), b"stub").unwrap();
    }

    let explicit = vec!["tes1002".to_string()];
    let mut files = collect_project_db_files(&root, Some(&explicit)).unwrap();
    files.sort();
    let names = files
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["tes1002", "tes1002_0001"]);

    // 没点名时维持 shadow 语义，不回灌主库。
    let unnamed = collect_project_db_files(&root, Some(&["tes9999".to_string()])).unwrap();
    assert_eq!(unnamed.len(), 1);
    assert!(unnamed[0].ends_with("tes1002_0001"));

    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn collect_project_db_files_rejects_sibling_extracts() {
    let root = make_temp_dir("dbfiles-sib");
    let db_dir = root.join("TES000");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::write(db_dir.join("tes9990_0001"), b"stub").unwrap();
    std::fs::write(db_dir.join("tes9990_0002"), b"stub").unwrap();
    let error =
        collect_project_db_files(&root, None).expect_err("sibling extracts must not be picked");
    assert!(error.to_string().contains("兄弟抽取"), "{error}");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn conflict_retry_backoff_grows_exponentially_and_caps() {
    for attempt in [1usize, 2, 3, 6, 12] {
        let base_ms = 25u64 << attempt.min(6);
        let waited = conflict_retry_backoff(attempt).as_millis() as u64;
        assert!(
            (base_ms..base_ms * 2).contains(&waited),
            "第 {attempt} 次重试等待 {waited}ms，期望落在 [{base_ms}, {})",
            base_ms * 2
        );
    }
    // 线性退避会让并发写入器同步重试、一起再撞上，必须是指数增长。
    assert!(conflict_retry_backoff(4).as_millis() >= conflict_retry_backoff(1).as_millis() * 2);
}

#[tokio::test]
async fn finish_write_pipeline_drains_writers_before_surfacing_parser_failure() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    const MESSAGES: usize = 8;
    let (sender, receiver) = flume::bounded::<SenderJsonsData>(MESSAGES);
    for i in 0..MESSAGES {
        sender
            .send(SenderJsonsData::PEJson(vec![format!("pe-{i}")]))
            .unwrap();
    }

    let consumed = Arc::new(AtomicUsize::new(0));
    let writer_consumed = consumed.clone();
    let mut handles = FuturesUnordered::new();
    handles.push(tokio::task::spawn(async move {
        // 先歇一会儿：不等待 writer 就返回的实现，此刻计数一定还是 0。
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        let mut seen = 0usize;
        while receiver.recv_async().await.is_ok() {
            seen += 1;
        }
        writer_consumed.store(seen, Ordering::SeqCst);
        (0usize, Vec::<String>::new())
    }));

    let error = finish_write_pipeline(sender, handles, Err(anyhow::anyhow!("parse boom")))
        .await
        .expect_err("解析失败必须向上报错，不能被当成同步成功");

    assert!(error.to_string().contains("parse boom"));
    assert_eq!(
        consumed.load(Ordering::SeqCst),
        MESSAGES,
        "解析失败时也要先排空 writer，否则已解析的数据会被静默丢弃"
    );
}

#[tokio::test]
async fn finish_write_pipeline_reports_writer_errors_when_parsing_succeeded() {
    let (sender, receiver) = flume::bounded::<SenderJsonsData>(1);
    let mut handles = FuturesUnordered::new();
    handles.push(tokio::task::spawn(async move {
        while receiver.recv_async().await.is_ok() {}
        (2usize, vec!["conflict on pe".to_string()])
    }));

    let error = finish_write_pipeline(sender, handles, Ok(()))
        .await
        .expect_err("写入失败不能被当成同步成功");
    assert!(error.to_string().contains("conflict on pe"));
    assert!(error.to_string().contains("2 time(s)"));
}

#[tokio::test]
async fn finish_write_pipeline_succeeds_when_parser_and_writers_are_clean() {
    let (sender, receiver) = flume::bounded::<SenderJsonsData>(1);
    let mut handles = FuturesUnordered::new();
    handles.push(tokio::task::spawn(async move {
        while receiver.recv_async().await.is_ok() {}
        (0usize, Vec::<String>::new())
    }));

    finish_write_pipeline(sender, handles, Ok(()))
        .await
        .unwrap();
}

#[test]
fn baseline_writer_failure_cleans_every_scheduled_dbnum() {
    assert_eq!(
        baseline_cleanup_targets([8000], [7997, 8000, 7999], false),
        BTreeSet::from([8000]),
        "局部 chunk 失败只清理已知失败库"
    );
    assert_eq!(
        baseline_cleanup_targets([], [7997, 8000, 7999], true),
        BTreeSet::from([7997, 8000, 7999]),
        "共享 writer 失败无法可靠归因，必须清理本批全部已调度库"
    );
}

#[test]
fn baseline_chunk_failure_stops_scheduling_and_wipes_only_after_writers_finish() {
    let source = include_str!("database.rs");
    let parser = source
        .split_once("let mut chunk_failed = false;")
        .expect("chunk failure state")
        .1
        .split_once("parsed_db_infos_for_parser")
        .expect("parsed result boundary")
        .0;
    assert!(parser.contains("chunk_failed = true;"));
    assert!(parser.contains("break;"), "失败后必须停止后续 chunk 调度");
    assert!(
        !parser.contains("expect(\"send attmap sql failed\")"),
        "属性发送失败也必须进入 chunk 硬门，不能 panic 后绕过失败库登记"
    );
    assert!(
        !include_str!("pe.rs").contains("expect(\"send pes error\")")
            && !include_str!("pe.rs").contains("expect(\"send pe_relates error\")"),
        "PE 与关系发送失败必须返回 Result 给基线清理路径"
    );
    assert!(
        parser.contains("if chunk_failed") && parser.contains("continue;"),
        "失败文件不得登记 parsed result"
    );

    let finish_at = source
        .find("let pipeline_result = finish_write_pipeline")
        .expect("writer drain");
    let wipe_at = source[finish_at..]
        .find("wipe_dbnum_for_reinit")
        .map(|offset| finish_at + offset)
        .expect("failed dbnum cleanup");
    assert!(
        finish_at < wipe_at,
        "必须先等待已派发写入，再清空失败 dbnum"
    );
}

#[test]
fn sync_pdms_awaits_global_meta_then_catalogue_then_design() {
    let source = include_str!("database.rs");
    let body = source
        .split_once("pub async fn sync_pdms")
        .expect("sync_pdms exists")
        .1
        .split_once("pub async fn define_dbnum_event")
        .expect("sync_pdms end exists")
        .0;
    let syst = body.find("&[\"SYST\", \"GLB\", \"GLOB\"]").unwrap();
    let dict = body.find("&[\"DICT\"]").unwrap();
    let cata = body.find("&[\"CATA\"]").unwrap();
    let desi = body.find("&[\"DESI\"]").unwrap();
    assert!(syst < dict && dict < cata && cata < desi);
    assert!(
        body[dict..cata].contains(".await"),
        "DICT must settle before Catalogue"
    );
    assert!(
        body[cata..desi].contains(".await"),
        "Catalogue must settle before Design"
    );
}

/// 统计行缺席时删 pe 不许炸，也不许凭空造一条统计行。
///
/// 现场（2026-08-18，test-increment 副本）：`sync_sys_only` 为提性能先
/// `REMOVE EVENT` 再写 pe，ams5100 于是有 236 条 pe、零条 `dbnum_info_table` 行；
/// 随后首次按需初始化要清库，`fast_delete` 的 Ref0 range DELETE 触发本事件，
/// 旧写法 `UPSERT ... SET count = count - 1 WHERE count > 0` 在缺行时走创建路径
/// （WHERE 拦不住），`NONE - 1` 把整条语句打死，批次 failed 并按相位门连坐阻断
/// 后面所有库。改回 UPSERT 或去掉 `?:0` 都会让本测试变红。
#[tokio::test(flavor = "multi_thread")]
async fn deleting_pe_rows_without_a_stats_row_neither_fails_nor_fabricates_one() {
    use surrealdb::engine::any::connect;

    #[derive(serde::Deserialize)]
    struct InfoRow {
        count: Option<i64>,
    }

    let db = connect("mem://").await.expect("mem boots");
    db.use_ns("dbnum_event")
        .use_db("missing_stats_row")
        .await
        .expect("use db");

    // 事件装载**之前**写入的 pe 行：与 sync 路径（先 REMOVE EVENT 再写）同形。
    db.query("CREATE pe:5100_1 SET dbnum = 5100, noun = 'GPRO', sesno = 3;")
        .await
        .expect("seed transport")
        .check()
        .expect("seed pe before the event exists");
    db.query(dbnum_event_sql())
        .await
        .expect("define event transport")
        .check()
        .expect("define event");

    let info: Vec<InfoRow> = db
        .query("SELECT count FROM dbnum_info_table;")
        .await
        .expect("stats transport")
        .take(0)
        .expect("decode stats");
    assert!(
        info.is_empty(),
        "前提：事件装载前写的 pe 行没有统计行，本测试才在验想验的东西"
    );

    db.query("DELETE pe:5100_1;")
        .await
        .expect("delete transport")
        .check()
        .expect("统计行缺席时删 pe 必须成功——NONE - 1 会把清库整条打死");

    let info: Vec<InfoRow> = db
        .query("SELECT count FROM dbnum_info_table;")
        .await
        .expect("stats transport")
        .take(0)
        .expect("decode stats");
    assert!(
        info.is_empty(),
        "缺席的统计行不许被删除事件凭空创建：那条行连 dbnum 都没有，\
         `DELETE dbnum_info_table WHERE dbnum = N` 清不掉它"
    );

    // 正常账仍要减：事件在场时创建的行有统计，删除后回到 0。
    db.query("CREATE pe:5100_2 SET dbnum = 5100, noun = 'GPRO', sesno = 4;")
        .await
        .expect("counted seed transport")
        .check()
        .expect("seed pe with the event live");
    let info: Vec<InfoRow> = db
        .query("SELECT count FROM dbnum_info_table;")
        .await
        .expect("stats transport")
        .take(0)
        .expect("decode stats");
    assert_eq!(
        info.first().and_then(|row| row.count),
        Some(1),
        "事件在场时创建的行必须记账"
    );

    db.query("DELETE pe:5100_2;")
        .await
        .expect("delete transport")
        .check()
        .expect("delete counted row");
    let info: Vec<InfoRow> = db
        .query("SELECT count FROM dbnum_info_table;")
        .await
        .expect("stats transport")
        .take(0)
        .expect("decode stats");
    assert_eq!(
        info.first().and_then(|row| row.count),
        Some(0),
        "有统计行时删除必须把计数减回去"
    );
}

/// 统计重算必须在服务端按 Ref0 聚合：**返回行数跟着 Ref0 走，不跟着 pe 行数走。**
///
/// 现场（2026-08-18，test-increment 副本）：旧写法 `SELECT record::id(id) AS key,
/// sesno FROM pe WHERE dbnum = N` 把整库拉回客户端，ams7351 的 3,345,853 行把 ws
/// 连接打死（`receiving from an empty and closed channel`），连带作废前面 2.6 小时
/// 的全量解析。回退成逐行回读会让本测试变红：那种语句一行 pe 出一行结果，既凑不出
/// `ref0` / `count` 字段，行数也会是 5 而不是 2。
#[tokio::test(flavor = "multi_thread")]
async fn stats_rebuild_aggregates_server_side_one_row_per_ref0() {
    use surrealdb::engine::any::connect;

    let db = connect("mem://").await.expect("mem boots");
    db.use_ns("dbnum_stats")
        .use_db("server_side_aggregate")
        .await
        .expect("use db");

    // 两个 Ref0、五行 pe，外加一行别的库（不许混进来）与一行缺 sesno 的历史行。
    db.query(
        "CREATE pe:23735_1 SET dbnum = 7351, sesno = 3; \
         CREATE pe:23735_900 SET dbnum = 7351, sesno = 116; \
         CREATE pe:23735_12 SET dbnum = 7351; \
         CREATE pe:25688_7 SET dbnum = 7351, sesno = 40; \
         CREATE pe:25688_4134 SET dbnum = 7351, sesno = 12; \
         CREATE pe:99999_1 SET dbnum = 8000, sesno = 230;",
    )
    .await
    .expect("seed transport")
    .check()
    .expect("seed pe");

    let groups: Vec<PeStatGroup> = db
        .query(pe_stat_groups_sql(7351))
        .await
        .expect("aggregate transport")
        .take(0)
        .expect("decode aggregate");

    assert_eq!(
        groups.len(),
        2,
        "聚合必须一个 Ref0 一行；行数等于 pe 行数就是又把整库拉回客户端了"
    );
    let mut by_ref0: BTreeMap<&str, &PeStatGroup> = BTreeMap::new();
    for group in &groups {
        by_ref0.insert(group.ref0.as_str(), group);
    }
    let first = by_ref0.get("23735").expect("Ref0 23735 分组");
    assert_eq!(first.count, 3);
    assert_eq!(first.max_sesno.unwrap_or_default(), 116);
    assert_eq!(
        first.max_ref1.unwrap_or_default(),
        900,
        "max_ref1 必须是数值最大值：`900` 与 `12` 按字符串比会取反"
    );
    let second = by_ref0.get("25688").expect("Ref0 25688 分组");
    assert_eq!(second.count, 2);
    assert_eq!(second.max_sesno.unwrap_or_default(), 40);
    assert_eq!(second.max_ref1.unwrap_or_default(), 4134);
}

/// 事件维护到位的统计不许被「顺手重算一遍」抹掉重写：只补身份字段，原地不动。
///
/// 现场（2026-08-18）：`sync_total_async_threaded` 这条路径**不摘事件**，ams7351
/// 的 pe 与统计都是 3,345,853、分毫不差，那次无条件重算没纠正任何东西，只贡献了
/// 一次把连接打死的全表回读。身份补写用 `UPDATE`（无 DELETE），所以事件写下的
/// `updated_at` 必须活着——退回无条件 `rebuild_dbnum_info_from_pe` 会先
/// `DELETE dbnum_info_table WHERE dbnum = N`，那一列当场消失，本测试变红。
#[tokio::test(flavor = "multi_thread")]
async fn stamping_identity_keeps_event_maintained_stats_in_place() {
    use surrealdb::engine::any::connect;

    #[derive(serde::Deserialize)]
    struct InfoRow {
        count: Option<i64>,
        sesno: Option<i64>,
        file_name: Option<String>,
        db_type: Option<String>,
        updated_at: Option<String>,
    }

    let db = connect("mem://").await.expect("mem boots");
    db.use_ns("dbnum_stats")
        .use_db("stamp_identity")
        .await
        .expect("use db");
    db.query(dbnum_event_sql())
        .await
        .expect("define event transport")
        .check()
        .expect("define event");

    // 事件在场时写 pe：统计由 CREATE 分支维护出来，带 updated_at、不带身份字段。
    db.query("CREATE pe:23735_1 SET dbnum = 7351, sesno = 3; CREATE pe:23735_2 SET dbnum = 7351, sesno = 116;")
        .await
        .expect("seed transport")
        .check()
        .expect("seed pe with the event live");

    let before: Vec<InfoRow> = db
        .query("SELECT * FROM dbnum_info_table WHERE dbnum = 7351;")
        .await
        .expect("stats transport")
        .take(0)
        .expect("decode stats");
    assert_eq!(before.len(), 1, "前提：事件已经维护出统计行");
    assert_eq!(before[0].count, Some(2));
    assert!(
        before[0].updated_at.is_some(),
        "前提：事件写的行带 updated_at，本测试靠它区分「原地补写」与「删了重建」"
    );
    assert!(
        before[0].file_name.is_none(),
        "前提：事件写不出身份字段，所以身份补写这一步确有必要"
    );

    assert_eq!(
        classify_stats_settlement(2, 1, 2),
        StatsSettlement::StampIdentity,
        "两侧对得上、统计行在场，就不该再付全量重算"
    );

    db.query(stamp_dbnum_info_identity_sql(7351, "ams7351_0001", "CATA"))
        .await
        .expect("stamp transport")
        .check()
        .expect("stamp identity");

    let after: Vec<InfoRow> = db
        .query("SELECT * FROM dbnum_info_table WHERE dbnum = 7351;")
        .await
        .expect("stats transport")
        .take(0)
        .expect("decode stats");
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].count, Some(2), "身份补写不许动 count");
    assert_eq!(after[0].sesno, Some(116), "身份补写不许动 sesno");
    assert_eq!(after[0].file_name.as_deref(), Some("ams7351_0001"));
    assert_eq!(after[0].db_type.as_deref(), Some("CATA"));
    assert_eq!(
        after[0].updated_at, before[0].updated_at,
        "统计行必须是原地更新的那一条——updated_at 变了就说明它被删掉重建过"
    );
}

/// 「对得上就跳过」不许把「一条都没记」也认成对得上。
#[test]
fn absent_or_mismatched_stats_still_pay_for_a_full_rebuild() {
    // 事件维护到位（2026-08-18 ams7351 现场形状）。
    assert_eq!(
        classify_stats_settlement(3_345_853, 1, 3_345_853),
        StatsSettlement::StampIdentity
    );
    // 统计整体缺席（`sync_sys_only` 摘着事件写出来的那批行，ams5100 现场形状）：
    // 两侧的和都不等，就算相等也不能跳过——没有统计行就是没记过。
    assert_eq!(
        classify_stats_settlement(236, 0, 0),
        StatsSettlement::Rebuild
    );
    // 空库：pe 与统计同为 0，仍要走重建，让残留的陈旧统计行被 DELETE 清掉。
    assert_eq!(classify_stats_settlement(0, 0, 0), StatsSettlement::Rebuild);
    // count 缺口（事件曾被坏版本覆盖）：这正是重建唯一能纠正的那类漏记。
    assert_eq!(
        classify_stats_settlement(100, 1, 99),
        StatsSettlement::Rebuild
    );
}
