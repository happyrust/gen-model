use aios_core::SUL_DB;
use aios_core::aios_db_mgr::aios_mgr::AiosDBMgr;
use aios_core::get_default_pdms_db_info;
use aios_core::helper::normalize_sql_string;
use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use aios_core::tool::db_tool::db1_dehash;
use aios_core::tool::hash_tool::hash_str;
use aios_core::types::*;
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
use std::collections::{BTreeMap, HashMap, HashSet};
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
use crate::tables::*;
use crate::versioned_db::member_prune;
use crate::versioned_db::pe::*;
use crate::versioned_db::task::get_global_db_sender;

const BASELINE_QUEUE_CAPACITY: usize = 100;
const BASELINE_WRITE_WINDOW: usize = 20;
const BASELINE_WRITE_WORKERS: usize = 4;

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
/// 返回 PermissionDenied，会让整个解析任务 panic。同名时 `_0001` 抽取库优先于基础库。
fn collect_project_db_files(project_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
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

    let mut file_map: HashMap<String, PathBuf> = HashMap::new();
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
        match file_name.strip_suffix("_0001") {
            Some(base_name) => {
                file_map.insert(base_name.to_string(), path);
            }
            None => {
                file_map.entry(file_name.to_string()).or_insert(path);
            }
        }
    }
    Ok(file_map.into_values().collect())
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

#[derive(Deserialize)]
struct PeStatRow {
    key: String,
    // Legacy/baseline PE rows can predate per-element session tracking.
    sesno: Option<i32>,
}

#[cfg(test)]
mod pe_stat_row_tests {
    use super::{PeStatRow, preserve_unparsed_pe_metadata};
    use aios_core::db::{DbBasicData, EleDataEntry};
    use aios_core::pdms_types::RefU64;
    use dashmap::DashMap;
    use std::collections::BTreeMap;

    #[test]
    fn legacy_null_session_is_accepted() {
        let row: PeStatRow =
            serde_json::from_value(serde_json::json!({"key": "24384_22403", "sesno": null}))
                .unwrap();

        assert_eq!(row.sesno.unwrap_or_default(), 0);
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
pub async fn rebuild_dbnum_info_from_pe(
    dbnum: u32,
    file_name: &str,
    db_type: &str,
) -> anyhow::Result<usize> {
    let mut response = SUL_DB
        .query(format!(
            "SELECT record::id(id) AS key, sesno FROM pe WHERE dbnum = {dbnum};"
        ))
        .await
        .map_err(|error| anyhow::anyhow!("read PE stats dbnum={dbnum} failed: {error}"))?
        .check()
        .map_err(|error| {
            anyhow::anyhow!("read PE stats dbnum={dbnum} statement failed: {error}")
        })?;
    let rows: Vec<PeStatRow> = response
        .take(0)
        .map_err(|error| anyhow::anyhow!("decode PE stats dbnum={dbnum} failed: {error}"))?;

    let mut by_ref0: BTreeMap<u64, (usize, i32, u64)> = BTreeMap::new();
    for row in &rows {
        let sesno = row.sesno.unwrap_or_default();
        let (ref0, ref1) = row
            .key
            .split_once('_')
            .ok_or_else(|| anyhow::anyhow!("invalid PE record id: {}", row.key))?;
        let ref0 = ref0.parse::<u64>()?;
        let ref1 = ref1.parse::<u64>()?;
        by_ref0
            .entry(ref0)
            .and_modify(|(count, max_sesno, max_ref1)| {
                *count += 1;
                *max_sesno = (*max_sesno).max(sesno);
                *max_ref1 = (*max_ref1).max(ref1);
            })
            .or_insert((1, sesno, ref1));
    }

    execute_surreal_checked(
        &format!("DELETE dbnum_info_table WHERE dbnum = {dbnum};"),
        &format!("reset dbnum_info_table dbnum={dbnum}"),
    )
    .await?;
    let file_name = file_name.replace('\'', "\\'");
    let db_type = db_type.replace('\'', "\\'");
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
    }
    Ok(rows.len())
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
        aios_core::define_owner_index().await.unwrap();
        aios_core::create_geom_index().await.unwrap();
        // aios_core::define_fullname_index().await.unwrap();
        aios_core::define_pe_index().await.unwrap();
    }
    if db_option.is_sync_history() {
        aios_core::define_ses_index().await.unwrap();
    }

    let mut dbno_set = Arc::new(DashSet::new());
    let mut create_tables_elapse = 0;
    // 执行多线程解析
    dbg!("执行多线程解析");
    let proj_progress_chunk = 80 / db_option.included_projects.len();
    // 遍历所有包含的项目
    for (proj_idx, project) in db_option.included_projects.iter().enumerate() {
        // gen-model-9 / ADR-007：SYS 元数据(SYST/DICT/GLB/GLOB)只解析「主项目」——
        // included_projects 的第一个即主项目。依赖项目(如 AvevaCatalogue)的 SYS 与主项目
        // 共用 dbnum 8191，若也解析会经 check_and_clear_db(8191) 把主项目刚写的设计 MDB 清掉、
        // 导致 get_world_refno 取不到设计库。故 SYS 仅在 proj_idx==0 解析；DESI/CATA 仍按各项目解析。
        let is_main_project = proj_idx == 0;
        let debug_refnos: Vec<RefU64> = db_option
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
        let cur_dbno_set = dbno_set.clone();
        if is_main_project && (is_debug || db_option.only_sync_sys || db_option.total_sync) {
            // let progress_sender = progress_sender.clone();
            match sync_total_async_threaded(
                &db_option,
                project,
                cur_dbno_set,
                &["DICT", "SYST", "GLB", "GLOB"],
                // progress_sender,
                proj_progress_chunk,
            )
            .await
            {
                Ok(_) => {
                    // 同步数据成功
                    println!("同步UDA和SYS数据成功。");
                }
                Err(e) => {
                    // 只打印会让「一条数据都没入库」的运行看起来是成功的，后续的 DESI 解析
                    // 又依赖这批 SYS 数据，必须直接失败。
                    return Err(e.context(format!("同步 {project} 的 UDA/SYS 数据失败")));
                }
            }
        }
        //只同步"DICT", "SYST", "GLB", "GLOB" 这些信息
        if db_option.only_sync_sys {
            continue;
        }
        // let progress_sender = progress_sender.clone();
        let cur_dbno_set = dbno_set.clone();
        match sync_total_async_threaded(
            &db_option,
            project,
            cur_dbno_set,
            &["DESI", "CATA"],
            // progress_sender,
            proj_progress_chunk,
        )
        .await
        {
            Ok(_) => {
                // 同步数据成功
                println!("同步数据成功。");
            }
            Err(e) => {
                return Err(e.context(format!("同步 {project} 的 DESI/CATA 数据失败")));
            }
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

pub async fn define_dbnum_event() -> anyhow::Result<()> {
    let event_sql = r#"
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
                UPSERT type::thing('dbnum_info_table', $ref_0) MERGE {
                    count: count - 1,
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
    "#;

    SUL_DB.query(event_sql).await?;

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
    let sql = format!(
        "SELECT value id FROM only pe WHERE dbnum = {} limit 1",
        db_no
    );
    let mut response = SUL_DB.query(&sql).await.expect("check db exists failed");
    use surrealdb::sql::Thing;
    let db_exists: Option<Thing> = response.take(0).unwrap();
    if db_exists.is_some() {
        println!(
            "Database with dbnum {} already exists in pe table. Will override with new data.",
            db_no
        );
        println!("开始删除已有的dbnum {db_no} 的数据");
        let sql = format!("delete array::flatten(select value ->pe_owner from pe where dbnum = {db_no});
                                    delete array::flatten(select value [refno, id] from pe where dbnum = {db_no});
                                   delete array::flatten(select value ->inst_relate from pe where dbnum = {db_no});
                                    ");
        SUL_DB.query(&sql).await.expect("clear db failed");
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
    // 与监控目录同一套解析（认 project_dirs 里的绝对 / UNC 路径），否则共享盘上的
    // 项目在解析这一步就会被拼成一个不存在的目录。
    let project_dir =
        crate::data_interface::project_paths::resolve_project_root(db_option, project)
            .ok_or_else(|| anyhow::anyhow!("无法解析项目目录: {project}"))?; // 创建一个Path对象，表示项目目录的路径
    dbg!(&project_dir);

    if !Path::new(&project_dir).exists() {
        dbg!("项目文件夹指定不正确");
        // 如果项目目录不存在，则抛出错误
        return Err(anyhow::anyhow!("项目文件夹指定不正确"));
    }
    let children_files = collect_project_db_files(Path::new(&project_dir))?;
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
    // SurrealDB 2.1 uses optimistic transactions. Unbounded same-table
    // concurrency caused silent write loss; one global writer was correct but
    // too slow for 7997. Use bounded concurrency plus checked conflict retries.
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
                                let sql = format!(
                                    "INSERT RELATION INTO pe_owner [{}]",
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
    let sync_versioned = db_option.sync_versioned.unwrap_or(false);

    let sender_clone = sender.clone();
    let parsed_db_infos = Arc::new(DashMap::<u32, (String, String, usize)>::new());
    let parsed_db_infos_for_parser = parsed_db_infos.clone();
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
                let mut file = File::open(&path).await.unwrap();
                let mut buf = vec![0u8; 60];
                file.read_exact(&mut buf).await.unwrap();
                let db_basic_info = parse_file_basic_info(&buf);
                let db_type = db_basic_info.db_type;
                let db_no = db_basic_info.db_no;
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
                let mut ses_range_map = BTreeMap::new();
                let mut sesno = 0;
                // let mut dt = Local::now().naive_local();
                {
                    let mut io = PdmsIO::new(&project, path.clone(), true);

                    //打开文件
                    if io.open().is_ok() {
                        //获取最新sesno
                        sesno = io.get_latest_sesno().unwrap_or_default();
                        if sesno > 0 {
                            // let sql = format!(
                            //     "
                            //     DELETE db_file_info:{0};
                            //     INSERT INTO db_file_info (id, db_type, sesno, dbnum, dt) VALUES ('{0}', '{1}', {2}, {3}, '{4}');",
                            //     &file_name, db_type, sesno, db_no, dt.and_utc().to_rfc3339()
                            // );
                            // SUL_DB.query(&sql).await.expect("save db_info failed");
                            // if sync_versioned {
                            //     continue;
                            // }
                        } else {
                            continue;
                        }
                        // 只保留最新数据：不再写历史/版本表（sessions/element_changes/pe_ses_h/ses/pe VERSION）。
                        // ses_range_map 已由 io.open() 构建，无需 store_all_refno_sesno_map。
                        //获取sesno range
                        ses_range_map = io.ses_range_map;
                    }
                }

                let project_name = project.as_str().to_string(); // 获取项目名称的字符串
                // 解析失败绝不能退化成“空库”：`unwrap_or_default()` 会让本文件以 0 元素
                // 计入 parsed_db_infos，基线层据此认定合法空库并推进 applied_sesno，
                // 于是整个 dbnum 被静默跳过。跳过本文件、不登记结果，让基线层以
                // “解析未返回目标文件结果”显式失败。
                let mut db_basic = match parse_file_db_basic_data(
                    &path,
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
                    save_pe_relates(&db_basic, sender_clone.clone()).await;
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
                for (chunk_index, chunk) in all_refnos.chunks(chunk_size).enumerate() {
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
                            if !preserved.is_empty() {
                                let samples = preserved
                                    .iter()
                                    .take(5)
                                    .map(ToString::to_string)
                                    .join(", ");
                                println!(
                                    "{file_name_clone}: {} 个元素完整属性解析失败，已保留 PE 拓扑元数据（样例: {samples}）",
                                    preserved.len()
                                );
                            }
                            //类型暂时不多线程
                            let total_attr_map_arc = Arc::new(total_attr_map);
                            total_cnt += total_attr_map_arc.len();
                            //开始执行保存数据
                            println!("开始保存pe数量: {}", total_attr_map_arc.len());
                            if !is_debug && is_save_db {
                                save_pes(
                                    &db_basic_clone,
                                    &total_attr_map_arc,
                                    db_no as i32,
                                    &file_name_clone,
                                    &db_type,
                                    &db_option_clone,
                                    sender_clone.clone(),
                                )
                                .await
                                .expect("save pe to surreal failed");
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
                                            sender_clone
                                                .send_async(SenderJsonsData::AttJson((
                                                    type_name.clone(),
                                                    json_vec,
                                                )))
                                                .await
                                                .expect("send attmap sql failed");
                                        }

                                        if !uda_json_vec.is_empty() {
                                            // dbg!(&uda_json_vec);
                                            sender_clone
                                                .send_async(SenderJsonsData::AttJson((
                                                    "ATT_UDA".to_string(),
                                                    uda_json_vec,
                                                )))
                                                .await
                                                .expect("send attmap sql failed");
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            dbg!(e.to_string());
                        }
                    }
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
    finish_write_pipeline(sender, insert_handles, parser_outcome).await?;
    if is_total_sync && is_save_db {
        for entry in parsed_db_infos.iter() {
            let dbnum = *entry.key();
            let (file_name, db_type, _) = entry.value();
            rebuild_dbnum_info_from_pe(dbnum, file_name, db_type).await?;
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

    let mut files = collect_project_db_files(&root).unwrap();
    files.sort();
    let names = files
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    assert_eq!(names, vec!["tes1002_0001", "tes1008"]);
    assert!(files.iter().all(|path| path.is_file()));

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
