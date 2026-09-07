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
use crate::versioned_db::task::get_global_db_sender;

const BASELINE_QUEUE_CAPACITY: usize = 100;
const BASELINE_WRITE_WINDOW: usize = 20;
// SurrealDB 2.1/RocksDB uses optimistic transactions for these multi-row
// INSERTs. Even disjoint PE ids update shared table/index state, so concurrent
// baseline writers can keep colliding until the bounded retry budget expires.
// Keep parsing concurrent and the channel bounded, but serialize persistence.
const BASELINE_WRITE_WORKERS: usize = 1;

pub enum SenderJsonsData {
    /// 一笔完整的元素批事务（`element_batch` Fresh 模式：每表一条 `INSERT IGNORE`，含 pe 统计观测）。
    /// 2026-09-06 起基线与增量共用同一个渲染器（plan 2026-09-06 §3 D2 / P2），此前的
    /// `PEJson / AttJson / PERelateJson` 三种按表拆开的消息随之退役。
    ElementBatch(String),
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

// `preserve_unparsed_pe_metadata`（属性解不出来就只留 `pe` 拓扑行）2026-09-04 随 legacy 基线
// 一起删除：e3d-io 基线与增量同一条失败闸——白名单外的解码失败让这一库的基线失败、清库、
// 不推水位（退役收口方案 Q3 甲），不再有「拓扑在、属性残」的中间态。

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

/// 补齐文件身份，保留显式批次维护的计数、会话号和更新时间。
fn stamp_dbnum_info_identity_sql(dbnum: u32, file_name: &str, db_type: &str) -> String {
    let file_name = file_name.replace('\'', "\\'");
    let db_type = db_type.replace('\'', "\\'");
    format!(
        "UPDATE dbnum_info_table SET file_name = '{file_name}', db_type = '{db_type}' \
         WHERE dbnum = {dbnum};"
    )
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
    use super::PeStatGroup;

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
}

/// 显式修复统计；重建和版本标记同事务提交，保留历史 Ref0 及最大编号。
/// 常态初始化/增量由 dbnum_stats 的批次事务维护，不调用全量重建。
pub async fn rebuild_dbnum_info_from_pe(
    dbnum: u32,
    file_name: &str,
    db_type: &str,
) -> anyhow::Result<usize> {
    let sql = format!(
        "BEGIN TRANSACTION;\n{}\n{}\nCOMMIT TRANSACTION;",
        crate::data_interface::dbnum_stats::rebuild_body(dbnum),
        stamp_dbnum_info_identity_sql(dbnum, file_name, db_type)
    );
    crate::data_interface::dbnum_stats::execute_on(&SUL_DB, &sql).await?;
    let (pe_count, _, info_count) = dbnum_stat_totals(dbnum).await?;
    anyhow::ensure!(
        pe_count == info_count,
        "PE statistics rebuild did not converge"
    );
    Ok(pe_count)
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

/// 基线写者已原子维护统计；结束时核验并补身份，不用全量重算掩盖差异。
async fn settle_dbnum_info_after_baseline(
    dbnum: u32,
    file_name: &str,
    db_type: &str,
) -> anyhow::Result<()> {
    crate::data_interface::dbnum_stats::initialize_on(&SUL_DB, dbnum).await?;
    let (pe_count, _, info_count) = dbnum_stat_totals(dbnum).await?;
    anyhow::ensure!(
        pe_count == info_count,
        "dbnum={dbnum} statistics mismatch: PE={pe_count}, statistics={info_count}"
    );
    execute_surreal_checked(
        &stamp_dbnum_info_identity_sql(dbnum, file_name, db_type),
        "stamp explicit PE statistics identity",
    )
    .await?;
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

/// 一个 `db_type` 在全部包含项目里的候选文件，按项目分组、经身份判重后的选中清单。
fn sys_metadata_files(
    db_option: &DbOption,
    db_type: &str,
) -> anyhow::Result<HashMap<String, Vec<String>>> {
    let mut candidates = Vec::new();
    for project in &db_option.included_projects {
        let project_dir =
            crate::data_interface::project_paths::resolve_project_root(db_option, project)
                .ok_or_else(|| anyhow::anyhow!("无法解析项目目录: {project}"))?;
        for path in collect_project_db_files(&project_dir, None)? {
            let mut header = [0u8; crate::data_interface::db_file_header::BASIC_INFO_HEADER_BYTES];
            std::fs::File::open(&path)
                .and_then(|mut file| file.read_exact(&mut header))
                .with_context(|| format!("读取候选数据库头失败: {}", path.display()))?;
            // 不是 dabacon 描述符页的文件旧读法会给空 `db_type`、落不进任何类型；这里直接跳过。
            let Some(info) =
                crate::data_interface::db_file_header::DbBasicInfo::from_header_bytes(&header)
            else {
                continue;
            };
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
            "{db_type} 清单身份阻断: {}",
            selection
                .blockers
                .iter()
                .map(|blocker| blocker.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    let selected_project_by_dbnum = selection
        .selected
        .iter()
        .map(|candidate| (candidate.dbnum, candidate.project.clone()))
        .collect::<HashMap<_, _>>();
    for candidate in selection.shadowed {
        println!(
            "[sys-bootstrap] {db_type} dbnum={} 的 {} 被 {} 遮蔽: {}",
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

/// 把配置项目的 SYS 元数据（SYST / COMM / MISC / GLB / GLOB 与 DICT）灌进一个**空库**。
///
/// 这是 `sync_pdms` 退役后唯一留下的那一半（ADR-058 D8）。留它的理由只有一个：DESI 解析要
/// 经 `MDB` / `WORL` 表反查世界 refno，一个从没同步过 SYS 的空库会让每个 dbnum 解出
/// 「只有根、0 个元素」。CATA 与 DESI 两道全局屏障随 `sync_pdms` 一起走了——目录不入持久层
/// （ADR-058），设计侧由数据批次 worker 按库首载。
///
/// 只有引导工具会调它（`bin/sync_sys_only`、l3 套件的空店引导）；**它不看任何配置开关**，
/// 调用即执行。
pub async fn bootstrap_sys_metadata(db_option: &DbOption) -> anyhow::Result<()> {
    if db_option.included_projects.is_empty() {
        return Err(anyhow::anyhow!("没有包含的项目"));
    }
    let started = tokio::time::Instant::now();
    println!("开始引导 {} 的 SYS 元数据", &db_option.project_name);

    retire_dbnum_event().await?;
    // 空库还没有这几个索引，而 SYS 灌完紧接着就是逐库首载。
    retry_surreal_write_operation("define owner index", aios_core::define_owner_index).await?;
    retry_surreal_write_operation("define geometry index", aios_core::create_geom_index).await?;
    retry_surreal_write_operation("define pe index", aios_core::define_pe_index).await?;

    let dbno_set = Arc::new(DashSet::new());
    let main_project = &db_option.included_projects[0];
    sync_total_async_threaded(
        db_option,
        main_project,
        dbno_set.clone(),
        &["SYST", "COMM", "MISC", "GLB", "GLOB"],
        100,
    )
    .await
    .with_context(|| format!("同步 {main_project} 的 SYST/COMM/MISC/GLB/GLOB 数据失败"))?;

    // DICT 属于 Meta，含依赖项目提供的 DICT；按显式优先级顺序处理，让共享裸 dbnum
    // 的判重确定性地留下权威文件。
    let dict_files = sys_metadata_files(db_option, "DICT")?;
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
        phase_option.included_db_files = Some(dict_files.get(project).cloned().unwrap_or_default());
        sync_total_async_threaded(&phase_option, project, dbno_set.clone(), &["DICT"], 100)
            .await
            .with_context(|| format!("同步 {project} 的 DICT 数据失败"))?;
    }

    println!(
        "SYS 元数据引导完成，耗时 {} ms",
        started.elapsed().as_millis()
    );
    Ok(())
}

/// Remove the retired trigger; all PE writers maintain statistics explicitly.
pub async fn retire_dbnum_event() -> anyhow::Result<()> {
    crate::data_interface::dbnum_stats::retire_event_on(&SUL_DB).await
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
    // Task-local state does not propagate through tokio::spawn. Carry the
    // worker's exact file token into the parser instead of capturing latest.
    let active_source = crate::data_interface::batch_worker::active_frozen_source()?;
    retire_dbnum_event().await?;
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
                        // 一笔元素批事务：pe / noun 行 / ATT_UDA / pe_owner 边全在里面（Fresh：只
                        // INSERT IGNORE，已有行赢——父层补缺重放叶子已写过的行与边时必须幂等，否则
                        // 重复 id 报错会把整次补缺同步打成失败），pe 统计观测同事务。
                        SenderJsonsData::ElementBatch(sql) => {
                            if let Err(error) =
                                crate::data_interface::dbnum_stats::execute_on(&SUL_DB, &sql).await
                            {
                                record_write_error(
                                    &mut error_count,
                                    &mut error_samples,
                                    "element batch",
                                    error,
                                );
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
            // gen-model-9 / ADR-007：SYS 元数据(SYST/DICT/COMM/MISC/GLB/GLOB)必须从其专属文件(amssys 等)解析，
            // 不应受 included_db_files 过滤——它列的是 DESI/CATA 数据库文件，从不含 SYS 文件。
            // SYS 同步始终遍历全部文件，再由下方 db_type 过滤只留 SYS 文件。DESI/CATA 同步(is_parse_sys=false)
            // 仍受 included_db_files 约束。
            if is_parse_sys
                || db_option_arc.included_db_files.is_none()
                || db_option_arc
                    .included_db_files
                    .as_ref()
                    .unwrap()
                    .contains(&file_name)
            {
                // dbg!(&file_name);
                // 基线冻结源（e3d-io 底座）：dbnum / 库类型 / 目标会话 / 冻结长度都从它取，
                // 收集与解码走 `stream_baseline`（与增量同一个 e3d-io 收集器）。
                let baseline_frozen =
                    match crate::versioned_db::baseline::source_for_file(
                        &path, None, active_source.as_ref(),
                    ) {
                        Ok(frozen) => frozen,
                        Err(error) => {
                            log::error!(
                                "打开基线 dabacon 冻结源失败 {}: {error:#}",
                                path.display()
                            );
                            continue;
                        }
                    };
                let db_type = baseline_frozen.db_type().to_owned();
                let db_no = baseline_frozen.dbnum() as u32;
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
                let sesno = baseline_frozen.target_sesno();
                if sesno == 0 {
                    continue;
                }

                // 基线 = 每个元素都是 `Add` 的退化窗口（ADR-058 D3 / D4；退役收口方案 §2）：
                // 收集走与增量同一个 e3d-io 收集器（`stream_baseline`），行形状走同一份
                // `ElementRows`，写者仍是下面那条有界队列 + 单写者的 `INSERT IGNORE` 批。
                // 解析失败绝不能退化成“空库”：不登记 parsed_db_infos、记进失败集，让基线层以
                // “解析未返回目标文件结果”显式失败并清库。
                let debug_refnos: Vec<RefU64> = db_option_arc
                    .debug_root_refnos
                    .as_ref()
                    .map(|x| {
                        x.iter()
                            .map(|x| RefU64::from_str(x).unwrap())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                //debug 不保存数据，只查看属性值
                let is_debug = !debug_refnos.is_empty();
                let save_rows = is_save_db && !is_debug;
                if is_save_db {
                    scheduled_baseline_dbnums_for_parser.insert(db_no);
                }
                // `ATT_UDA` 的 `u` 链接要把 UKEY 换成 `UDA:<refno>`；DICT 相先于 DESI 相，
                // 定义此时已在库里。查不到的键记账、随文件回执打一行，不静默丢。
                let uda_keys = match crate::data_interface::uda_keys::UdaKeyMap::load().await {
                    Ok(keys) => Arc::new(keys),
                    Err(error) => {
                        log::error!(
                            "baseline UDA key table failed: file={file_name} dbnum={db_no}: {error:#}"
                        );
                        failed_baseline_dbnums_for_parser.insert(db_no);
                        continue;
                    }
                };
                // CATA 依赖 seed（plan 2026-09-06 §3 D6 (b)）：流式收集时顺手攒本库每个 DESI 元素的
                // 目录链引用，文件收尾一次闭包、经同一条写者队列入库（单写者不变），排在
                // `finalize_baseline` 之前——它在 `sync_total_async_threaded` 返回、写者排空之后才跑。
                let dependency_seeds =
                    std::sync::Arc::new(std::sync::Mutex::new(std::collections::BTreeSet::<RefU64>::new()));
                let outcome = {
                    let frozen = baseline_frozen.clone();
                    let db_type = db_type.clone();
                    let sender = sender_clone.clone();
                    let uda_keys = uda_keys.clone();
                    let debug_refnos = debug_refnos.clone();
                    let dependency_seeds = dependency_seeds.clone();
                    // 文件 I/O + 解码是同步的 CPU 活，放阻塞池；有界队列的同步 `send` 只阻塞
                    // 这条阻塞线程，写者任务照常在运行时上消费。
                    tokio::task::spawn_blocking(move || {
                        crate::data_interface::e3d_collector::stream_baseline(
                            &frozen,
                            &db_type,
                            chunk_size,
                            |chunk| {
                                for ignored in &chunk.ignored {
                                    crate::data_interface::parse_error::note_element_failure(
                                        &ignored.refno.to_string(),
                                        &ignored.reason,
                                    );
                                }
                                if is_debug {
                                    for (_, element) in &chunk.elements {
                                        if debug_refnos.contains(&element.refno) {
                                            dbg!(element);
                                        }
                                    }
                                }
                                if !save_rows {
                                    return Ok(());
                                }
                                if db_type.eq_ignore_ascii_case("DESI") {
                                    let seeds = crate::data_interface::cata_dependency::seeds_from_elements(
                                        chunk.elements.iter().map(|(_, element)| element),
                                    );
                                    dependency_seeds
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                                        .extend(seeds);
                                }
                                // 与增量路同一个渲染器（`element_batch`，Fresh）：每笔事务就是
                                // 一条消息，切块按 `tx_limits`（`pe_chunk` / `att_chunk` 不再参与）。
                                let (transactions, _report) =
                                    crate::versioned_db::baseline::render_baseline_batches(
                                        &chunk,
                                        db_no as i32,
                                        &uda_keys,
                                    )?;
                                for sql in transactions {
                                    sender
                                        .send(SenderJsonsData::ElementBatch(sql))
                                        .map_err(|error| {
                                            anyhow::anyhow!("baseline dispatch failed: {error}")
                                        })?;
                                }
                                Ok(())
                            },
                        )
                    })
                    .await
                };
                let stats = match outcome {
                    Ok(Ok(stats)) => stats,
                    Ok(Err(error)) => {
                        log::error!("baseline failed: file={file_name} dbnum={db_no}: {error:#}");
                        failed_baseline_dbnums_for_parser.insert(db_no);
                        println!(
                            "{file_name}: 基线失败，停止该文件后续调度；等待写入收口后清理 dbnum={db_no}: {error:#}"
                        );
                        continue;
                    }
                    Err(join_error) => {
                        log::error!(
                            "baseline task panicked: file={file_name} dbnum={db_no}: {join_error}"
                        );
                        failed_baseline_dbnums_for_parser.insert(db_no);
                        println!(
                            "{file_name}: 基线任务异常终止；等待写入收口后清理 dbnum={db_no}: {join_error}"
                        );
                        continue;
                    }
                };
                // CATA 依赖闭包（plan 2026-09-06 §3 D6 (b)）：本库 DESI 元素依赖的目录链元素，
                // 经同一条写者队列 Fresh 入库；闭包缺失（库不在 MDB / 元素不在）只出声，
                // 解析器本身出错按基线失败处理（缺依赖的基线不算落地，与稳态窗口同纪律）。
                let dependency_seeds = std::mem::take(
                    &mut *dependency_seeds
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                );
                if save_rows && !dependency_seeds.is_empty() {
                    let dependency = async {
                        let rows = crate::data_interface::cata_dependency::resolve_dependency_rows(
                            project.as_str(),
                            db_no,
                            &dependency_seeds,
                        )
                        .await?;
                        let batches = crate::data_interface::cata_dependency::render_dependency_transactions(
                            &rows,
                            &uda_keys,
                            crate::data_interface::element_batch::WriteMode::Fresh,
                        )?;
                        for (cata_dbnum, sqls) in &batches {
                            let elements = rows.by_dbnum.get(cata_dbnum).map(Vec::len).unwrap_or(0);
                            let sesno = rows
                                .by_dbnum
                                .get(cata_dbnum)
                                .and_then(|elements| elements.first())
                                .map(|(sesno, _)| *sesno)
                                .unwrap_or(0);
                            for sql in sqls {
                                sender_clone
                                    .send_async(SenderJsonsData::ElementBatch(sql.clone()))
                                    .await
                                    .map_err(|error| anyhow::anyhow!("dependency dispatch failed: {error}"))?;
                            }
                            sender_clone
                                .send_async(SenderJsonsData::ElementBatch(
                                    crate::data_interface::cata_dependency::render_dependency_state(
                                        *cata_dbnum,
                                        sesno,
                                        elements,
                                    ),
                                ))
                                .await
                                .map_err(|error| anyhow::anyhow!("dependency state dispatch failed: {error}"))?;
                        }
                        anyhow::Ok(rows)
                    }
                    .await;
                    match dependency {
                        Ok(rows) => {
                            println!(
                                "{file_name}: CATA 依赖闭包 seeds={} → {} 行 / {} 库，缺失 {}",
                                dependency_seeds.len(),
                                rows.elements(),
                                rows.by_dbnum.len(),
                                rows.manifest.missing
                            );
                            if rows.manifest.missing > 0 {
                                log::warn!(
                                    "{file_name}: CATA 依赖闭包缺失 {} 个 refno（库不在 MDB / 元素不在），样例 {:?}",
                                    rows.manifest.missing,
                                    rows.manifest.missing_samples.iter().take(5).collect::<Vec<_>>()
                                );
                            }
                        }
                        Err(error) => {
                            log::error!(
                                "baseline dependency closure failed: file={file_name} dbnum={db_no}: {error:#}"
                            );
                            failed_baseline_dbnums_for_parser.insert(db_no);
                            println!(
                                "{file_name}: CATA 依赖闭包失败，基线不算落地；等待写入收口后清理 dbnum={db_no}: {error:#}"
                            );
                            continue;
                        }
                    }
                }
                // 新路没有「属性残、拓扑在」的中间态（Q3 甲）：跑到这里就是这一文件的
                // 完整属性都解出来了，销掉旧进程可能留下的 ATTRS 账。
                crate::data_interface::parse_error::note_attrs_success(&file_name);
                if let Some(warning) = uda_keys.misses_warning(db_no) {
                    log::warn!("{file_name}: {warning}");
                    println!("{file_name}: {warning}");
                }
                if stats.ignored > 0 {
                    println!(
                        "{file_name}: {} 条字典无属性 schema 的系统记录跳过（已记进解析失败账本）",
                        stats.ignored
                    );
                }
                println!(
                    "解析任务完成, 耗时: {} s, 总数量: {}（索引 {} 条，WORL {} 个，{} 块，读页 {}）",
                    time.elapsed().as_secs_f32(),
                    stats.elements,
                    stats.indexed,
                    stats.worlds,
                    stats.chunks,
                    stats.pages_read
                );
                parsed_db_infos_for_parser
                    .insert(db_no, (file_name.clone(), db_type.clone(), stats.elements));
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
    if is_save_db {
        for entry in parsed_db_infos.iter() {
            let dbnum = *entry.key();
            let (file_name, db_type, _) = entry.value();
            settle_dbnum_info_after_baseline(dbnum, file_name, db_type).await?;
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
            .send(SenderJsonsData::ElementBatch(format!(
                "BEGIN TRANSACTION; INSERT IGNORE INTO pe [{{ id: pe:1_{i} }}] RETURN NONE; COMMIT TRANSACTION;"
            )))
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

/// 基线失败不许静默（宪法 III）。legacy 解析段用一个 `chunk_failed` 标志收口，
/// e3d-io 退化窗口（B8.3）改成「每条失败路登记失败 dbnum 后立刻 `continue`」——
/// 钉的仍是同三件事：失败被归因到具体 dbnum、失败文件不登记 parsed result、
/// 清库只发生在已派发写入收口之后。
#[test]
fn baseline_failure_registers_the_dbnum_and_wipes_only_after_writers_finish() {
    let source = include_str!("database.rs");
    let scheduled = source
        .split_once("scheduled_baseline_dbnums_for_parser.insert(db_no);")
        .expect("baseline scheduling site")
        .1
        .split_once("parsed_db_infos_for_parser")
        .expect("parsed result boundary")
        .0;

    let registered = scheduled
        .matches("failed_baseline_dbnums_for_parser.insert(db_no);")
        .count();
    assert!(
        registered >= 3,
        "UDA 键表、基线收集、收集任务 panic 三条失败路各要登记一次失败 dbnum，实得 {registered}"
    );
    assert_eq!(
        scheduled.matches("continue;").count(),
        registered,
        "登记了失败就必须 continue：失败文件不得再往下登记 parsed result"
    );
    assert!(
        scheduled.contains(".send(SenderJsonsData::ElementBatch(sql))")
            && scheduled.contains(".map_err(|error| {"),
        "队列满 / 写者退出必须作为 Err 冒出 chunk sink，让 stream_baseline 中止并归因到本库"
    );
    assert!(
        !scheduled.contains(".expect("),
        "基线调度段不许 panic：panic 绕过失败 dbnum 登记，会留下一个半写的库"
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

/// legacy 全量解析入口与它的四个开关一起退役（ADR-058 D8）。原来这里钉的是
/// 「Meta → Catalogue → Design 三道全局屏障的先后」，那是 `sync_pdms` 自己的编排；
/// direct 成为默认形态后它一次也没被走到过（仓里每份配置都没打开这四个键），
/// 而基线摄入早已只剩 [`sync_total_async_threaded`] 一个入口——由 `manual_update`
/// 的首载 / 重建路径按**单个文件**调用，屏障由数据批次队列负责。
///
/// 现在钉的是「别再长回来」：本文件不许重新出现那个入口，也不许再读那四个开关。
#[test]
fn the_legacy_full_sync_entry_point_stays_retired() {
    let source = include_str!("database.rs");
    let entry = format!("pub async fn {}", "sync_pdms");
    assert!(
        !source.contains(&entry),
        "legacy 全量解析入口已退役（ADR-058 D8），不许复活"
    );
    for switch in [
        "total_sync",
        "incr_sync",
        "only_sync_sys",
        "is_sync_history",
    ] {
        assert!(
            !source.contains(&format!("db_option.{switch}")),
            "legacy 全量开关 {switch} 已退役，本文件不许再读它"
        );
    }
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
