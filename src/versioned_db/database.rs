use aios_core::SUL_DB;
use aios_core::aios_db_mgr::aios_mgr::AiosDBMgr;
use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use aios_core::types::*;
use serde::Deserialize;
#[cfg(feature = "sql")]
use sqlx::{Connection, MySql, MySqlPool, Pool};
#[cfg(feature = "sql")]
use sqlx::{Error, Executor};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::mem::take;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::fs::{File, create_dir_all};

use crate::consts::*;
use crate::data_interface::tidb_manager::AiosDBManager;
// use crate::graph_db::pdms_arango::*;
use crate::tables::*;

/// 列出项目 `*000` 目录下需要解析的库文件。
///
/// 只保留普通文件：目录名不含 `.` 时会混进来，而 Windows 上 `File::open` 打开目录
/// 返回 PermissionDenied，会让整个解析任务 panic。抽取家族归并后只保留叶子
/// （主库被遮蔽）；同号兄弟抽取拒绝静默挑选。
/// `explicit_files`：调用方点名要解析的文件名（`included_db_files` 口径）。抽取家族
/// 归并会把被叶子 shadow 的主库从清单里删掉；但父层补缺（ADR-028 第 6 条）恰恰要
/// 点名解析主库——被点名的 shadow 文件必须回到清单，否则补缺同步静默空转。
pub(crate) fn collect_project_db_files(
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
pub(crate) fn stamp_dbnum_info_identity_sql(dbnum: u32, file_name: &str, db_type: &str) -> String {
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
pub(crate) async fn dbnum_stat_totals(dbnum: u32) -> anyhow::Result<(usize, usize, usize)> {
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

/// legacy 全量解析入口与它的四个开关一起退役（ADR-058 D8）。原来这里钉的是
/// 「Meta → Catalogue → Design 三道全局屏障的先后」，那是 `sync_pdms` 自己的编排；
/// direct 成为默认形态后它一次也没被走到过（仓里每份配置都没打开这四个键）。
/// 同日晚些时候它的底座 `sync_total_async_threaded`（多类型 / 多文件 / `included_db_files`
/// 过滤的 legacy 壳）也退役了：基线摄入只剩 `versioned_db::baseline_ingest::ingest_file`
/// 一个按**单个文件**的入口，由 `manual_update` 的首载 / 重建路径调用，屏障由数据批次队列
/// 负责。最后一截 `bootstrap_sys_metadata`（空库 SYS 引导，`bin/sync_sys_only` 的全部内容）
/// 同日一并退役：SYS 元数据不是一道单独的引导，而是初始化的一部分——服务首轮重扫把它们
/// 当普通库入队首载（`needs_initial_load` 明说「SYS meta 也算」），规格工具走
/// `AiosDBManager::initialize_project_sys_metadata`，两条路同一个 `initialize_dbnum_baseline`。
///
/// 现在钉的是「别再长回来」：本文件不许重新出现那三个入口，也不许再读那四个开关。
#[test]
fn the_legacy_full_sync_entry_point_stays_retired() {
    let source = include_str!("database.rs");
    for retired in [
        "sync_pdms",
        "sync_total_async_threaded",
        "bootstrap_sys_metadata",
    ] {
        let entry = format!("pub async fn {retired}");
        assert!(
            !source.contains(&entry),
            "legacy 全量解析入口 {retired} 已退役（ADR-058 D8 / 2026-09-07 基线摄入方案），不许复活"
        );
    }
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
