#[cfg(feature = "surreal-save")]
use aios_core::SUL_DB;

// 内存KV数据库全局连接（从 aios_core 导入）
#[cfg(feature = "mem-kv-save")]
#[allow(unused_imports)]
use aios_core::SUL_MEM_DB;


use aios_core::get_default_pdms_db_info;
use aios_core::helper::normalize_sql_string;
use aios_core::options::DbOption;
#[cfg(feature = "sql")]
use aios_core::db_pool::get_global_pool;
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
#[cfg(feature = "sql")]
use sea_orm::{ConnectionTrait, Schema, Statement};
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
use crate::versioned_db::pe::*;

pub enum SenderJsonsData {
    PEJson(Vec<String>),
    PERelateJson(Vec<String>),
    AttJson((String, Vec<String>)),
    // 项目名 , sql
    MysqlSql((String, String)),
    // 新增：用于更新dbnum_info_table
    DbnumInfoUpdate(Vec<String>),
    // 新增：用于按db_num分表保存简化的PE数据 (table_name, sql)
    PartitionedPEJson { table_name: String, sql: String },
    // Kuzu 数据: Vec<(PE, NamedAttrMap)>
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
pub async fn create_info_database(db_option: &DbOption) -> anyhow::Result<()> {
    let pool = get_global_pool(db_option).await?;
    let project_name = db_option.project_name.clone();
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
    let pool = aios_mgr.get_project_pool().await?;
    let result = pool.execute(gen_create_element_tables_sql().as_str()).await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(&e);
        }
    }

    Ok(())
}

/// 带进度回调的同步pdms数据到数据库
pub async fn sync_pdms_with_callback<F>(
    db_option: &DbOption,
    mut progress_callback: Option<F>,
) -> anyhow::Result<()>
where
    F: FnMut(&str, usize, usize, usize, usize, usize, usize) + Send,
{
    if db_option.included_projects.is_empty() {
        return Err(anyhow::anyhow!("没有包含的项目"));
    }

    // 开始同步pdms/E3D项目的数据
    println!("开始同步pdms/E3D: {} 的数据", &db_option.project_name);
    let mut time = tokio::time::Instant::now();

    #[cfg(feature = "surreal-save")]
    {
        // 解析前移除EVENT，防止大量的event触发
        println!("正在移除dbnum_event以提高解析性能...");
        let remove_event_sql = "REMOVE EVENT update_dbnum_event ON pe;";
        match SUL_DB.query(remove_event_sql).await {
            Ok(_) => println!("成功移除update_dbnum_event"),
            Err(e) => println!("移除update_dbnum_event失败（可能不存在）: {:?}", e),
        }
    }

    // 创建表
    let create_tables_start = time.elapsed().as_millis();
    // TODO: 需要实现create_tables函数或使用现有的表创建逻辑
    // create_tables().await?;
    let create_tables_elapse = time.elapsed().as_millis() - create_tables_start;

    let mut dbno_set = Arc::new(DashSet::new());

    // 执行多线程解析
    dbg!("执行多线程解析");
    let proj_progress_chunk = 80 / db_option.included_projects.len();
    let total_projects = db_option.included_projects.len();

    // 遍历所有包含的项目
    for (project_index, project) in db_option.included_projects.iter().enumerate() {
        // 解析时不应该受 debug_model_refnos 影响，只用于模型生成调试
        let debug_refnos: Vec<RefU64> = Vec::new(); // 暂时禁用解析调试模式

        // 统计项目中的文件数量
        let project_dir = db_option.get_project_path(&project).unwrap();
        let total_files = if Path::new(&project_dir).exists() {
            let target_dir = std::fs::read_dir(&project_dir)
                .unwrap()
                .into_iter()
                .map(|entry| {
                    let entry = entry.unwrap();
                    entry.path()
                })
                .find(|x| x.is_dir() && x.file_name().unwrap().to_str().unwrap().ends_with("000"))
                .unwrap();

            let children_files: Vec<PathBuf> = std::fs::read_dir(target_dir)?
                .into_iter()
                .map(|entry| {
                    let entry = entry.unwrap();
                    entry.path()
                })
                .collect();

            // 处理文件名_0001和文件名同时存在的情况
            let mut file_map = HashMap::new();
            for path in children_files.iter() {
                let file_name = path.file_stem().unwrap().to_str().unwrap();
                if let Some(base_name) = file_name.strip_suffix("_0001") {
                    file_map.insert(base_name.to_string(), path.clone());
                } else {
                    if !file_map.contains_key(file_name) {
                        file_map.insert(file_name.to_string(), path.clone());
                    }
                }
            }
            file_map.len()
        } else {
            0
        };

        // 通知进度回调开始处理项目
        if let Some(ref mut callback) = progress_callback {
            callback(
                project,
                project_index + 1,
                total_projects,
                0,
                total_files,
                0,
                0,
            );
        }

        //debug 不保存数据，只复杂查看属性值
        let is_debug = !debug_refnos.is_empty();
        let cur_dbno_set = dbno_set.clone();
        if is_debug || db_option.only_sync_sys || db_option.total_sync {
            match sync_total_async_threaded_with_callback(
                &db_option,
                project,
                cur_dbno_set,
                &["DICT", "SYST", "GLB", "GLOB"],
                proj_progress_chunk,
                &mut progress_callback,
                project_index + 1,
                total_projects,
            )
            .await
            {
                Ok(_) => {
                    println!("同步UDA和SYS数据成功。");
                }
                Err(e) => {
                    println!("{}", e.to_string());
                }
            }
        }

        //只同步"DICT", "SYST", "GLB", "GLOB" 这些信息
        if db_option.only_sync_sys {
            continue;
        }

        let cur_dbno_set = dbno_set.clone();
        match sync_total_async_threaded_with_callback(
            &db_option,
            project,
            cur_dbno_set,
            &["DESI", "CATA"],
            proj_progress_chunk,
            &mut progress_callback,
            project_index + 1,
            total_projects,
        )
        .await
        {
            Ok(_) => {
                println!("同步数据成功。");
            }
            Err(e) => {
                println!("{}", e.to_string());
            }
        }
    }

    // 解析完成后重新定义EVENT
    println!("正在重新定义dbnum_event...");
    match aios_core::define_dbnum_event().await {
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

    #[cfg(feature = "surreal-save")]
    {
        // 解析前移除EVENT，防止大量的event触发
        println!("正在移除dbnum_event以提高解析性能...");
        let remove_event_sql = "REMOVE EVENT update_dbnum_event ON pe;";
        match SUL_DB.query(remove_event_sql).await {
            Ok(_) => println!("成功移除update_dbnum_event"),
            Err(e) => println!("移除update_dbnum_event失败（可能不存在）: {:?}", e),
        }
    }

    // 获取默认的数据库连接字符串
    if db_option.sync_tidb.unwrap_or(false) {
        #[cfg(feature = "sql")]
        {
            create_info_database(db_option).await?;
        }
    }

    //只有重新同步时，才需要定义index
    let enable_index = db_option.total_sync || db_option.enable_index.unwrap_or(true);
    if enable_index {
        // 主库创建索引
        aios_core::define_owner_index().await.unwrap();
        aios_core::create_geom_index().await.unwrap();
        // aios_core::define_fullname_index().await.unwrap();
        aios_core::define_pe_index().await.unwrap();

        // 备份内存KV库也创建相同索引（幂等）
        #[cfg(feature = "mem-kv-save")]
        {
            use aios_core::SUL_MEM_DB;
            // 使用新增的带连接版本
            let _ = aios_core::rs_surreal::index::define_owner_index_with(&SUL_MEM_DB).await;
            let _ = aios_core::rs_surreal::index::create_geom_index_with(&SUL_MEM_DB).await;
            let _ = aios_core::rs_surreal::index::define_pe_index_with(&SUL_MEM_DB).await;
        }
    }
    if db_option.is_sync_history() {
        aios_core::define_ses_index().await.unwrap();
        #[cfg(feature = "mem-kv-save")]
        {
            use aios_core::SUL_MEM_DB;
            let _ = aios_core::rs_surreal::index::define_ses_index_with(&SUL_MEM_DB).await;
        }
    }

    let mut dbno_set = Arc::new(DashSet::new());
    let mut create_tables_elapse = 0;
    // 执行多线程解析
    dbg!("执行多线程解析");
    let proj_progress_chunk = 80 / db_option.included_projects.len();
    // 遍历所有包含的项目
    for project in &db_option.included_projects {
        // 解析时不应该受 debug_model_refnos 影响，只用于模型生成调试
        let debug_refnos: Vec<RefU64> = Vec::new(); // 暂时禁用解析调试模式
        //debug 不保存数据，只复杂查看属性值
        let is_debug = !debug_refnos.is_empty();
        let cur_dbno_set = dbno_set.clone();
        if is_debug || db_option.only_sync_sys || db_option.total_sync {
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
                    // 同步数据失败，打印错误信息
                    println!("{}", e.to_string());
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
                // 同步数据失败，打印错误信息
                println!("{}", e.to_string());
            }
        }
    }

    // 解析完成后重新定义EVENT
    println!("正在重新定义dbnum_event...");
    match aios_core::define_dbnum_event().await {
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

#[cfg(feature = "surreal-save")]
#[deprecated(
    note = "已迁移到 aios_core::define_dbnum_event，请使用 aios_core::define_dbnum_event() 代替"
)]
pub async fn define_dbnum_event() -> anyhow::Result<()> {
    // 调用 aios_core 中的实现
    aios_core::define_dbnum_event().await
}

#[cfg(not(feature = "surreal-save"))]
#[deprecated(
    note = "已迁移到 aios_core::define_dbnum_event，请使用 aios_core::define_dbnum_event() 代替"
)]
pub async fn define_dbnum_event() -> anyhow::Result<()> {
    aios_core::define_dbnum_event().await
}

/// 定义dbnum_info_table的更新事件, pe 的id 为array的情况
#[cfg(feature = "surreal-save")]
pub async fn define_dbnum_event_array_id() -> anyhow::Result<()> {
    let event_sql = r#"
DEFINE EVENT OVERWRITE update_dbnum_event ON pe WHEN $event = "CREATE" OR $event = "UPDATE" OR $event = "DELETE" THEN {
            -- 获取当前记录的 dbnum
            LET $dbnum = $value.dbnum;
            LET $id = record::id($value.id);
            let $ref_0 = array::at($id, 0);
            let $ref_1 = array::at($id, 1);
            let $is_delete = $value.deleted and $event = "UPDATE";
            let $max_sesno = if $after.sesno > $before.sesno?:0 { $after.sesno } else { $before.sesno };
            -- 根据事件类型处理  type::record("dbnum_info_table", $ref_0)
            IF $event = "CREATE"   {
                UPSERT type::record('dbnum_info_table', $ref_0) MERGE {
                    dbnum: $dbnum,
                    count: count?:0 + 1,
                    sesno: $max_sesno,
                    max_ref1: $ref_1
                };
            } ELSE IF $event = "DELETE" OR $is_delete  {
                UPSERT type::record('dbnum_info_table', $ref_0) MERGE {
                    count: count - 1,
                    sesno: $max_sesno,
                    max_ref1: $ref_1
                }
                WHERE count > 0;
            };
        };
    "#;

    SUL_DB.query(event_sql).await?;
    Ok(())
}

#[cfg(not(feature = "surreal-save"))]
pub async fn define_dbnum_event_array_id() -> anyhow::Result<()> {
    Ok(())
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

#[cfg(feature = "surreal-save")]
pub async fn check_and_clear_db(db_no: u32) -> anyhow::Result<()> {
    let sql = format!(
        "SELECT value id FROM only pe WHERE dbnum = {} limit 1",
        db_no
    );
    let mut response = SUL_DB.query(&sql).await.expect("check db exists failed");
    use serde_json::Value as JsonValue;
    let records: Vec<JsonValue> = response.take(0).unwrap_or_default();
    if !records.is_empty() {
        println!(
            "Database with dbnum {} already exists in pe table. Will override with new data.",
            db_no
        );
        println!("开始删除已有的dbnum {db_no} 的数据");
        let sql = format!("delete array::flatten(select value ->pe_owner from pe where dbnum = {db_no});
                                    delete array::flatten(select value [refno, id] from pe where dbnum = {db_no});
                                    ");
        SUL_DB.query(&sql).await.expect("clear db failed");
    }
    Ok(())
}

#[cfg(not(feature = "surreal-save"))]
pub async fn check_and_clear_db(_db_no: u32) -> anyhow::Result<()> {
    Ok(())
}

/// 带进度回调的多线程同步数据
pub async fn sync_total_async_threaded_with_callback<F>(
    db_option: &DbOption,
    project: &str,
    cur_dbno_set: Arc<DashSet<u32>>,
    db_types: &[&str],
    proj_progress_chunk: usize,
    progress_callback: &mut Option<F>,
    current_project: usize,
    total_projects: usize,
) -> anyhow::Result<()>
where
    F: FnMut(&str, usize, usize, usize, usize, usize, usize) + Send,
{
    println!("开始解析 {project} 的 {:?}", db_types);
    let db_option_arc = Arc::new(db_option.clone());

    let project_dir = db_option.get_project_path(&project).unwrap();

    if !Path::new(&project_dir).exists() {
        dbg!("项目文件夹指定不正确");
        return Err(anyhow::anyhow!("项目文件夹指定不正确"));
    }

    // 获取并统计文件
    let mut children_files = {
        let target_dir = std::fs::read_dir(&project_dir)
            .unwrap()
            .into_iter()
            .map(|entry| {
                let entry = entry.unwrap();
                entry.path()
            })
            .find(|x| x.is_dir() && x.file_name().unwrap().to_str().unwrap().ends_with("000"))
            .unwrap();
        std::fs::read_dir(target_dir)?
            .into_iter()
            .map(|entry| {
                let entry = entry.unwrap();
                entry.path()
            })
            .collect::<Vec<PathBuf>>()
    };

    // 处理文件名_0001和文件名同时存在的情况
    let mut file_map = HashMap::new();
    for path in children_files.iter() {
        let file_name = path.file_stem().unwrap().to_str().unwrap();
        if let Some(base_name) = file_name.strip_suffix("_0001") {
            file_map.insert(base_name.to_string(), path.clone());
        } else {
            if !file_map.contains_key(file_name) {
                file_map.insert(file_name.to_string(), path.clone());
            }
        }
    }

    children_files = file_map.into_values().collect();
    let total_files = children_files.len();

    // 通知进度回调文件统计完成
    if let Some(callback) = progress_callback {
        callback(
            project,
            current_project,
            total_projects,
            0,
            total_files,
            0,
            0,
        );
    }

    // 继续原有的处理逻辑...
    let project = Arc::new(project.to_string());
    let mut is_replace = db_option_arc.replace_dbs;
    let replace_types = db_option_arc.replace_types.clone();
    let b_replace_types = replace_types.is_some();
    let b_save_mysql = db_option_arc.sync_tidb.unwrap_or(false);
    if b_replace_types {
        is_replace = true;
    }
    let chunk_size = db_option_arc.sync_chunk_size.unwrap_or(10_0000) as usize;

    const CHUNK_SIZE: usize = 100;
    let (sender, receiver) = flume::unbounded();

    // 启动数据库写入任务
    let mut insert_handles = FuturesUnordered::new();
    for i in 0..16 {
        let receiver: flume::Receiver<SenderJsonsData> = receiver.clone();
        #[cfg(feature = "sql")]
        let pool = AiosDBManager::get_project_pool().await.unwrap().clone();

        let insert_handle = tokio::task::spawn(async move {
            // 使用 ready_chunks 而不是 chunks，这样可以在 channel 关闭时立即处理剩余数据
            use futures::stream::StreamExt;
            let mut record_stream = receiver.into_stream().ready_chunks(200);
            while let Some(stream) = record_stream.next().await {
                for data in stream {
                    match data {
                        #[cfg(feature = "surreal-save")]
                        SenderJsonsData::PEJson(pes) => {
                            if !pes.is_empty() {
                                dbg!(pes.len());
                                let sql = format!("INSERT IGNORE INTO pe [{}]", pes.join(","));

                                // 保存到主数据库
                                let mut response =
                                    SUL_DB.query(&sql).await.expect("insert pes failed");

                                // 如果启用了 mem-kv-save，同时保存到备份数据库
                                #[cfg(feature = "mem-kv-save")]
                                {
                                    match SUL_MEM_DB.query(&sql).await {
                                        Ok(_) => {}
                                        Err(e) => {
                                            log::warn!("保存PE到内存KV数据库失败: {}", e);
                                        }
                                    }
                                }
                            }
                        }
                        #[cfg(not(feature = "surreal-save"))]
                        SenderJsonsData::PEJson(pes) => {
                            let _ = pes;
                        }
                        #[cfg(feature = "surreal-save")]
                        SenderJsonsData::PERelateJson(relates) => {
                            if !relates.is_empty() {
                                let sql = format!(
                                    "INSERT RELATION INTO pe_owner [{}]",
                                    relates.join(",")
                                );

                                // 保存到主数据库
                                SUL_DB.query(&sql).await.expect("insert pe_owner failed");

                                // 如果启用了 mem-kv-save，同时保存到备份数据库
                                #[cfg(feature = "mem-kv-save")]
                                {
                                    match SUL_MEM_DB.query(&sql).await {
                                        Ok(_) => {}
                                        Err(e) => {
                                            log::warn!("保存PE关系到内存KV数据库失败: {}", e);
                                        }
                                    }
                                }
                            }
                        }
                        #[cfg(not(feature = "surreal-save"))]
                        SenderJsonsData::PERelateJson(relates) => {
                            let _ = relates;
                        }
                        #[cfg(feature = "surreal-save")]
                        SenderJsonsData::AttJson((type_name, jsons)) => {
                            if !jsons.is_empty() {
                                let sql = format!(
                                    "INSERT IGNORE INTO {} [{}]",
                                    type_name,
                                    jsons.join(",")
                                );
                                SUL_DB.query(sql).await.expect("insert att failed");
                            }
                        }
                        #[cfg(not(feature = "surreal-save"))]
                        SenderJsonsData::AttJson((_type_name, jsons)) => {
                            let _ = jsons;
                        }
                        #[cfg(feature = "surreal-save")]
                        SenderJsonsData::DbnumInfoUpdate(sqls) => {
                            for sql in sqls {
                                SUL_DB.query(sql).await.expect("update dbnum_info failed");
                            }
                        }
                        #[cfg(not(feature = "surreal-save"))]
                        SenderJsonsData::DbnumInfoUpdate(sqls) => {
                            let _ = sqls;
                        }
                        #[cfg(feature = "surreal-save")]
                        SenderJsonsData::PartitionedPEJson { table_name, sql } => {
                            // 保存简化PE数据到分表
                            log::debug!("插入到分表 {}", table_name);
                            SUL_DB
                                .query(&sql)
                                .await
                                .expect("insert partitioned pe failed");

                            // 如果启用了 mem-kv-save，同时保存到备份数据库
                            #[cfg(feature = "mem-kv-save")]
                            {
                                match SUL_MEM_DB.query(&sql).await {
                                    Ok(_) => {}
                                    Err(e) => {
                                        log::warn!(
                                            "保存分表PE到内存KV数据库失败: {} | 表: {}",
                                            e,
                                            table_name
                                        );
                                    }
                                }
                            }
                        }
                        #[cfg(not(feature = "surreal-save"))]
                        SenderJsonsData::PartitionedPEJson { table_name, sql } => {
                            let _ = (table_name, sql);
                        }
                        #[cfg(feature = "sql")]
                        SenderJsonsData::MySqlJson((table_name, jsons)) => {
                            if b_save_mysql && !jsons.is_empty() {
                                let sql = format!(
                                    "INSERT IGNORE INTO {} VALUES {}",
                                    table_name,
                                    jsons.join(",")
                                );
                                match sqlx::query(&sql).execute(&pool).await {
                                    Ok(_) => {}
                                    Err(e) => {
                                        dbg!(e.to_string());
                                    }
                                }
                            }
                        }
                        SenderJsonsData::MysqlSql((table_name, sql)) => {
                            // 处理MySQL SQL语句
                            #[cfg(feature = "sql")]
                            if b_save_mysql {
                                match sqlx::query(&sql).execute(&pool).await {
                                    Ok(_) => {}
                                    Err(e) => {
                                        dbg!(e.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
        insert_handles.push(insert_handle);
    }
    // ========== 文件解析与写库主循环（带进度回调） ==========
    // 为保持与非回调版本一致，这里直接内联主循环，避免将 progress_callback 移入子任务造成生命周期/Send 限制。

    // 与非回调版本保持一致的控制参数
    let db_types_clone = db_types.iter().map(|&x| x.to_string()).collect::<Vec<_>>();
    let is_parse_sys = db_types_clone.contains(&"SYST".to_string());
    #[cfg(feature = "surreal-save")]
    let is_save_db = db_option.is_save_db();
    #[cfg(not(feature = "surreal-save"))]
    let is_save_db = false;
    let is_sync_history = db_option.is_sync_history();
    let is_total_sync = db_option.total_sync;
    let sync_versioned = db_option.sync_versioned.unwrap_or(false);

    for (file_idx, path) in children_files.into_iter().enumerate() {
        let total_files = total_files; // 仅为语义清晰
        let file_name = path.file_name().unwrap().to_str().unwrap().to_string();
        if file_name.contains('.') {
            // 进入文件（将其计入进度），随即跳过
            if let Some(cb) = progress_callback.as_mut() {
                cb(
                    project.as_str(),
                    current_project,
                    total_projects,
                    file_idx + 1,
                    total_files,
                    0,
                    0,
                );
            }
            continue;
        }

        // 进入文件 - 上报当前文件号
        if let Some(cb) = progress_callback.as_mut() {
            cb(
                project.as_str(),
                current_project,
                total_projects,
                file_idx + 1,
                total_files,
                0,
                0,
            );
        }

        let dbno_set = cur_dbno_set.clone();
        let mut time = Instant::now();

        // 读取文件头，判定 db_type / db_no
        let mut file = File::open(&path).await.unwrap();
        let mut buf = vec![0u8; 60];
        file.read_exact(&mut buf).await.unwrap();
        let db_basic_info = parse_file_basic_info(&buf);
        let db_type = db_basic_info.db_type;
        let db_no = db_basic_info.db_no;

        if is_replace {
            check_and_clear_db(db_no).await.unwrap();
        }
        // 类型过滤
        if !db_types_clone.contains(&db_type) {
            // 依然汇报一次该文件完成
            if let Some(cb) = progress_callback.as_mut() {
                cb(
                    project.as_str(),
                    current_project,
                    total_projects,
                    file_idx + 1,
                    total_files,
                    0,
                    0,
                );
            }
            continue;
        }
        // 避免重复
        if dbno_set.contains(&db_no) {
            if let Some(cb) = progress_callback.as_mut() {
                cb(
                    project.as_str(),
                    current_project,
                    total_projects,
                    file_idx + 1,
                    total_files,
                    0,
                    0,
                );
            }
            continue;
        }
        dbno_set.insert(db_no);

        // 读取 sesno、存储 refno->sesno map
        let mut ses_range_map = BTreeMap::new();
        let mut sesno = 0;
        {
            let mut io = PdmsIO::new(&project, path.clone(), true);
            if io.open().is_ok() {
                sesno = io.get_latest_sesno().unwrap_or_default();
                if sesno == 0 {
                    if let Some(cb) = progress_callback.as_mut() {
                        cb(
                            project.as_str(),
                            current_project,
                            total_projects,
                            file_idx + 1,
                            total_files,
                            0,
                            0,
                        );
                    }
                    continue;
                }
                if is_sync_history {
                    io.sync_history().await.unwrap();
                    if let Some(cb) = progress_callback.as_mut() {
                        cb(
                            project.as_str(),
                            current_project,
                            total_projects,
                            file_idx + 1,
                            total_files,
                            0,
                            0,
                        );
                    }
                    continue;
                } else {
                    io.store_all_refno_sesno_map().await.unwrap();
                }
                ses_range_map = io.ses_range_map;
            } else {
                if let Some(cb) = progress_callback.as_mut() {
                    cb(
                        project.as_str(),
                        current_project,
                        total_projects,
                        file_idx + 1,
                        total_files,
                        0,
                        0,
                    );
                }
                continue;
            }
        }

        let project_name = project.as_str().to_string();
        let mut db_basic =
            parse_file_db_basic_data(&path, &file_name, project_name.as_str()).unwrap_or_default();
        let all_refnos = db_basic.children_map.keys().cloned().collect::<Vec<_>>();
        let total_chunks = std::cmp::max(1, (all_refnos.len() + chunk_size - 1) / chunk_size);

        let db_basic = Arc::new(db_basic);
        if is_save_db {
            save_pe_relates(&db_basic, sender.clone()).await;
        }
        // 解析时不应该受 debug_model_refnos 影响，只用于模型生成调试
        // 如果需要调试解析过程，应该使用独立的 debug_parse_refnos 配置
        let debug_refnos: Vec<RefU64> = Vec::new(); // 暂时禁用解析调试模式
        let is_debug = !debug_refnos.is_empty();
        if is_debug {
            if let Some(children) = db_basic.children_map.get(&debug_refnos[0]) {
                dbg!(children);
            }
        }
        let debug_refnos = Arc::new(debug_refnos);

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
                    let total_attr_map_arc = Arc::new(total_attr_map);
                    total_cnt += total_attr_map_arc.len();
                    let should_save = !is_debug && is_save_db;
                    if should_save {
                        save_pes(
                            &db_basic_clone,
                            &total_attr_map_arc,
                            db_no as i32,
                            &file_name_clone,
                            &db_type,
                            &db_option_clone,
                            sender.clone(),
                        )
                        .await
                        .expect("save pes failed");
                    }
                    // UDA 类型写入
                    for kv in type_ele_map.iter() {
                        let noun: i32 = *kv.key() as _;
                        let type_name = db1_dehash(noun as _);
                        if type_name.is_empty() {
                            continue;
                        }
                        for refnos in &kv.value().iter().chunks(db_option_clone.att_chunk as _) {
                            let mut json_vec = vec![];
                            let mut uda_json_vec = vec![];
                            for refno in refnos {
                                let att = total_attr_map_arc.get(refno).unwrap();
                                if is_debug {
                                    if debug_refnos.contains(&att.get_refno_or_default().refno()) {
                                        dbg!(att.value());
                                    } else {
                                        continue;
                                    }
                                }
                                if !is_save_db {
                                    continue;
                                }
                                if let Some(json) = att.gen_sur_json() {
                                    json_vec.push(json);
                                }
                                if let Some(json) = att.gen_sur_json_uda(&[]) {
                                    uda_json_vec.push(normalize_sql_string(&json));
                                }
                            }
                            if is_save_db {
                                if !json_vec.is_empty() {
                                    sender
                                        .send(SenderJsonsData::AttJson((
                                            type_name.clone(),
                                            json_vec,
                                        )))
                                        .expect("send attmap sql failed");
                                }
                                if !uda_json_vec.is_empty() {
                                    sender
                                        .send(SenderJsonsData::AttJson((
                                            "ATT_UDA".to_string(),
                                            uda_json_vec,
                                        )))
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

            // 分块进度
            if let Some(cb) = progress_callback.as_mut() {
                cb(
                    project.as_str(),
                    current_project,
                    total_projects,
                    file_idx + 1,
                    total_files,
                    chunk_index + 1,
                    total_chunks,
                );
            }
        }

        println!(
            "解析任务完成, 耗时: {} s, 总数量: {}",
            time.elapsed().as_secs_f32(),
            total_cnt
        );
        // 文件完成：若无分块也至少回报一次
        if let Some(cb) = progress_callback.as_mut() {
            cb(
                project.as_str(),
                current_project,
                total_projects,
                file_idx + 1,
                total_files,
                total_chunks,
                total_chunks,
            );
        }
    }

    // 等待所有写入任务完成
    drop(sender);
    while let Some(_result) = insert_handles.next().await {
        // 可在此加入错误处理或日志
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
) -> anyhow::Result<()> {
    println!("开始解析 {project} 的 {:?}", db_types);
    let db_option_arc = Arc::new(db_option.clone()); // 创建一个Arc对象，表示数据库选项

    let project_dir = db_option.get_project_path(&project).unwrap(); // 创建一个Path对象，表示项目目录的路径
    dbg!(&project_dir);

    if !Path::new(&project_dir).exists() {
        dbg!("项目文件夹指定不正确");
        // 如果项目目录不存在，则抛出错误
        return Err(anyhow::anyhow!("项目文件夹指定不正确"));
    }
    let mut children_files = {
        // 获取子文件列表
        let target_dir = std::fs::read_dir(&project_dir)
            .unwrap()
            .into_iter()
            .map(|entry| {
                let entry = entry.unwrap();
                entry.path()
            })
            .find(|x| x.is_dir() && x.file_name().unwrap().to_str().unwrap().ends_with("000"))
            .unwrap();
        std::fs::read_dir(target_dir)?
            .into_iter()
            .map(|entry| {
                let entry = entry.unwrap();
                entry.path()
            })
            .collect::<Vec<PathBuf>>()
    };
    // 处理文件名_0001和文件名同时存在的情况
    let mut file_map = HashMap::new();
    for path in children_files.iter() {
        let file_name = path.file_stem().unwrap().to_str().unwrap();
        if let Some(base_name) = file_name.strip_suffix("_0001") {
            file_map.insert(base_name.to_string(), path.clone());
        } else {
            // 只有当没有_0001版本时才插入普通版本
            if !file_map.contains_key(file_name) {
                file_map.insert(file_name.to_string(), path.clone());
            }
        }
    }

    // 更新children_files只包含需要处理的文件
    children_files = file_map.into_values().collect();
    // println!("需要处理的文件: {:?}", &children_files);
    // dbg!(children_files.len());
    // 先解析一遍uda
    // 正式解析
    
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
    // #[cfg(feature = "sql")]
    // let pool = mgr.get_project_pools().await?;

    const CHUNK_SIZE: usize = 100;
    // let (sender, receiver) = flume::bounded(CHUNK_SIZE);
    let (sender, receiver) = flume::unbounded();
    let mut insert_handles = FuturesUnordered::new();
    for i in 0..16 {
        let receiver: flume::Receiver<SenderJsonsData> = receiver.clone();
        #[cfg(feature = "sql")]
        let pool = AiosDBManager::get_project_pool().await.unwrap().clone();

        let insert_handle = tokio::task::spawn(async move {
            // 使用 ready_chunks 而不是 chunks，这样可以在 channel 关闭时立即处理剩余数据
            use futures::stream::StreamExt;
            let mut record_stream = receiver.into_stream().ready_chunks(200);
            // let mut cnt = 0;
            while let Some(stream) = record_stream.next().await {
                // while let Ok(data) = receiver.recv_async().await {
                for data in stream {
                    match data {
                        #[cfg(feature = "surreal-save")]
                        SenderJsonsData::PEJson(pes) => {
                            if !pes.is_empty() {
                                let sql = format!("INSERT IGNORE INTO pe [{}]", pes.join(","));

                                // 保存到主数据库
                                let mut response =
                                    SUL_DB.query(&sql).await.expect("insert pes failed");

                                // 如果启用了 mem-kv-save，同时保存到备份数据库
                                #[cfg(feature = "mem-kv-save")]
                                {
                                    match SUL_MEM_DB.query(&sql).await {
                                        Ok(_) => {}
                                        Err(e) => {
                                            log::warn!("保存PE到内存KV数据库失败: {}", e);
                                        }
                                    }
                                }
                            }
                        }
                        #[cfg(not(feature = "surreal-save"))]
                        SenderJsonsData::PEJson(pes) => {
                            let _ = pes;
                        }
                        #[cfg(feature = "surreal-save")]
                        SenderJsonsData::PERelateJson(relates) => {
                            if !relates.is_empty() {
                                let sql = format!(
                                    "INSERT RELATION INTO pe_owner [{}]",
                                    relates.join(",")
                                );

                                // 保存到主数据库
                                SUL_DB.query(&sql).await.expect("insert pe_owner failed");

                                // 如果启用了 mem-kv-save，同时保存到备份数据库
                                #[cfg(feature = "mem-kv-save")]
                                {
                                    match SUL_MEM_DB.query(&sql).await {
                                        Ok(_) => {}
                                        Err(e) => {
                                            log::warn!("保存PE关系到内存KV数据库失败: {}", e);
                                        }
                                    }
                                }
                            }
                        }
                        #[cfg(not(feature = "surreal-save"))]
                        SenderJsonsData::PERelateJson(relates) => {
                            let _ = relates;
                        }
                        #[cfg(feature = "surreal-save")]
                        SenderJsonsData::AttJson((table, atts)) => {
                            if !atts.is_empty() {
                                let sql =
                                    format!("INSERT IGNORE INTO {} [{}]", table, atts.join(","));

                                // 保存到主数据库
                                SUL_DB.query(&sql).await.expect("insert atts failed");

                                // 如果启用了 mem-kv-save，同时保存到备份数据库
                                #[cfg(feature = "mem-kv-save")]
                                {
                                    // match SUL_MEM_DB.query(&sql).await {
                                    //     Ok(_) => {},
                                    //     Err(e) => {
                                    //         log::warn!("保存属性到内存KV数据库失败: {}", e);
                                    //     }
                                    // }
                                }
                            }
                        }
                        #[cfg(not(feature = "surreal-save"))]
                        SenderJsonsData::AttJson((table, atts)) => {
                            let _ = (table, atts);
                        }
                        #[cfg(feature = "surreal-save")]
                        SenderJsonsData::DbnumInfoUpdate(updates) => {
                            if !updates.is_empty() {
                                // 使用UPSERT语法来更新或插入dbnum_info_table记录
                                for update in updates {
                                    SUL_DB
                                        .query(update.as_str())
                                        .await
                                        .expect("upsert dbnum_info failed");

                                    // 同步到内存KV备份库
                                    #[cfg(feature = "mem-kv-save")]
                                    {
                                        if let Err(e) = SUL_MEM_DB.query(update.as_str()).await {
                                            log::warn!(
                                                "保存DbnumInfo到内存KV数据库失败: {} | SQL: {}",
                                                e,
                                                update
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        #[cfg(not(feature = "surreal-save"))]
                        SenderJsonsData::DbnumInfoUpdate(updates) => {
                            let _ = updates;
                        }
                        #[cfg(feature = "surreal-save")]
                        SenderJsonsData::PartitionedPEJson { table_name, sql } => {
                            // 保存简化PE数据到分表
                            log::debug!("插入到分表 {}", table_name);
                            SUL_DB
                                .query(&sql)
                                .await
                                .expect("insert partitioned pe failed");

                            // 如果启用了 mem-kv-save，同时保存到备份数据库
                            #[cfg(feature = "mem-kv-save")]
                            {
                                match SUL_MEM_DB.query(&sql).await {
                                    Ok(_) => {}
                                    Err(e) => {
                                        log::warn!(
                                            "保存分表PE到内存KV数据库失败: {} | 表: {}",
                                            e,
                                            table_name
                                        );
                                    }
                                }
                            }
                        }
                        #[cfg(not(feature = "surreal-save"))]
                        SenderJsonsData::PartitionedPEJson { table_name, sql } => {
                            let _ = (table_name, sql);
                        }
                        #[cfg(feature = "sql")]
                        SenderJsonsData::MysqlSql((project, sql)) => {
                            // let Some(pool) = pools_clone.get(&project) else {
                            //     continue;
                            // };
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
            // if cnt > 0 {
            //     println!("thread {i} Imported records: {}", cnt);
            // }
        });
        insert_handles.push(insert_handle);
    }
    let db_types_clone = db_types
        .into_iter()
        .map(|&x| x.to_string())
        .collect::<Vec<_>>();
    let is_parse_sys = db_types_clone.contains(&"SYST".to_string());
    #[cfg(feature = "surreal-save")]
    let is_save_db = db_option.is_save_db();
    #[cfg(not(feature = "surreal-save"))]
    let is_save_db = false;
    let is_sync_history = db_option.is_sync_history();
    let is_total_sync = db_option.total_sync;
    let sync_versioned = db_option.sync_versioned.unwrap_or(false);

    let sender_clone = sender.clone();
    let children_files_len = children_files.len();
    let db_file_progress_chunk = (proj_progress_chunk as f32 / children_files_len as f32) as usize;
    // let progress_sender_clone = progress_sender.clone();
    tokio::spawn(async move {
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
            if (is_parse_sys && is_total_sync)
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
                //需要检查pe里是否有这个dbno，如果有，则需要改成使用upsert
                if is_replace {
                    check_and_clear_db(db_no).await.unwrap();
                }
                //如果不是全部解析，需要检查类型，全部解析一定要解析syst等配置文件数据库
                if !db_types_clone.contains(&db_type) {
                    continue;
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
                        if is_sync_history {
                            //同步历史纪录
                            io.sync_history().await.unwrap();
                            //同步完历史纪录就返回
                            continue;
                        } else {
                            //存储所有refno sesno map
                            io.store_all_refno_sesno_map().await.unwrap();
                        }
                        //获取sesno range
                        ses_range_map = io.ses_range_map;
                    }
                }

                let project_name = project.as_str().to_string(); // 获取项目名称的字符串
                let mut db_basic =
                    parse_file_db_basic_data(&path, &file_name, project_name.clone().as_str())
                        .unwrap_or_default();
                let all_refnos = db_basic.children_map.keys().cloned().collect::<Vec<_>>();

                let db_basic = Arc::new(db_basic);
                if is_save_db {
                    save_pe_relates(&db_basic, sender_clone.clone()).await;
                }
                // 解析时不应该受 debug_model_refnos 影响，只用于模型生成调试
                let debug_refnos: Vec<RefU64> = Vec::new(); // 暂时禁用解析调试模式
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
                            //类型暂时不多线程
                            let total_attr_map_arc = Arc::new(total_attr_map);
                            total_cnt += total_attr_map_arc.len();
                            //开始执行保存数据
                            println!("开始保存pe数量: {}", total_attr_map_arc.len());
                            let should_save = !is_debug && is_save_db;
                            if should_save {
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
                                .expect("save pes failed");
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
                                    &sender_clone,
                                )
                                .await;
                            }
                            for kv in type_ele_map.iter() {
                                let noun: i32 = *kv.key() as _;
                                let type_name = db1_dehash(noun as _);
                                if type_name.is_empty() {
                                    continue;
                                }
                                //UDA 还是要单独存，不然数据很容易混乱
                                for refnos in
                                    &kv.value().iter().chunks(db_option_clone.att_chunk as _)
                                {
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
                                                .send(SenderJsonsData::AttJson((
                                                    type_name.clone(),
                                                    json_vec,
                                                )))
                                                .expect("send attmap sql failed");
                                        }

                                        if !uda_json_vec.is_empty() {
                                            // dbg!(&uda_json_vec);
                                            sender_clone
                                                .send(SenderJsonsData::AttJson((
                                                    "ATT_UDA".to_string(),
                                                    uda_json_vec,
                                                )))
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
    })
    .await
    .unwrap();
    drop(sender);
    // insert_handles.push(parse_handle);
    while let Some(result) = insert_handles.next().await {
        // 处理每个完成的 future 的结果
        // dbg!(&result);
    }
    // all_handles.push(parse_handle);
    // futures::future::join_all(take(&mut all_handles)).await;
    // futures::future::join_all(&mut [parse_handle]).await;
    Ok(())
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
