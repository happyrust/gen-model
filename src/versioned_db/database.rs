use aios_core::get_default_pdms_db_info;
use aios_core::options::DbOption;
use aios_core::pdms_types::*;
use aios_core::tool::db_tool::db1_dehash;
use aios_core::types::*;
use aios_core::SUL_DB;
use dashmap::{DashMap, DashSet};
use futures::StreamExt;
use itertools::Itertools;
use parse_pdms_db::parse::*;
use petgraph::prelude::DiGraph;
#[cfg(feature = "sql")]
use sea_orm::{ConnectionTrait, Schema, Statement};
#[cfg(feature = "sql")]
use sqlx::{Connection, MySql, MySqlPool, Pool};
#[cfg(feature = "sql")]
use sqlx::{Error, Executor};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::hash::Hash;
use std::io::Read;
use std::mem::take;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use aios_core::aios_db_mgr::aios_mgr::AiosDBMgr;
use aios_core::tool::hash_tool::hash_str;
use tokio::fs;
use tokio::io::AsyncReadExt;
// use std::time::Instant;
use tokio::fs::{create_dir_all, File};
use tokio::time::Instant;

use crate::consts::*;
use crate::data_interface::tidb_manager::AiosDBManager;
// use crate::graph_db::pdms_arango::*;
use crate::tables::*;
use crate::versioned_db::database::SenderSql::SurrealSql;
use crate::versioned_db::pe::*;

pub enum SenderSql {
    SurrealSql(String),
    // 项目名 , sql
    MysqlSql((String, String)),
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
    let pool = aios_mgr.get_global_pool().await?;
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
    let pool = aios_mgr.get_project_pool().await?;
    let result = pool
        .execute(gen_create_element_tables_sql().as_str())
        .await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(&e);
        }
    }

    Ok(())
}

/// 初始化同步pdms数据到数据
pub async fn sync_pdms(db_option: &DbOption) -> anyhow::Result<()> {
    // 开始同步pdms/E3D项目的数据
    println!("开始同步pdms/E3D: {} 的数据", &db_option.project_name);
    // 计时器开始
    let mut time = tokio::time::Instant::now();
    // 获取默认的数据库连接字符串
    let aios_mgr = AiosDBMgr::init_from_db_option().await?;
    if db_option.sync_tidb.unwrap_or(false) {
        #[cfg(feature = "sql")]
        create_info_database(&aios_mgr).await?;
    }

    if !db_option.incr_sync {
        aios_core::define_owner_index().await.unwrap();
        aios_core::create_geom_index().await.unwrap();
        // aios_core::define_fullname_index().await.unwrap();
        aios_core::define_pe_index().await.unwrap();
    }

    let mut create_tables_elapse = 0;
    // 执行多线程解析
    dbg!("执行多线程解析");
    // 遍历所有包含的项目
    for project in &db_option.included_projects {
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

        if is_debug || (!db_option.incr_sync) {
            match sync_total_async_threaded(&db_option, project, &["DICT", "SYST", "GLB", "GLOB"])
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

        match sync_total_async_threaded(&db_option, project, &["DESI", "CATA"]).await {
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

    // 输出创建表所花费的时间
    println!("创建表花费时间: {} ms", create_tables_elapse);
    // 输出初始化数据库所花费的时间
    println!(
        "初始化数据库时间: {} ms",
        time.elapsed().as_millis() - create_tables_elapse
    );

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
                    if error.code() == Some(Cow::from("42000")) {} else {
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

//分成两部分，一部分先保存UDA 和 SYS 这些数据
///多线程同步数据，包括增量同步
pub async fn sync_total_async_threaded(
    db_option: &DbOption,
    project: &str,
    db_types: &[&str],
) -> anyhow::Result<()> {
    let pg_dir = "assets/pg";
    create_dir_all(pg_dir).await.unwrap();
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
    // dbg!(children_files.len());
    // 先解析一遍uda
    // 正式解析
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
        let pool = mgr.get_project_pools().await?;
    const CHUNK_SIZE: usize = 10000;
    let (sender, receiver) = flume::bounded(CHUNK_SIZE);

    let mut all_handles = vec![];
    for i in 0..60 {
        let receiver: flume::Receiver<SenderSql> = receiver.clone();
        #[cfg(feature = "sql")]
            let pools_clone = pool.clone();

        let insert_handle = tokio::task::spawn(async move {
            let mut record_stream = receiver.into_stream().chunks(CHUNK_SIZE);
            while let Some(sqls) = record_stream.next().await {
                println!("thread {i} Imported records: {}", sqls.len());
                for sql in sqls {
                    match sql {
                        SenderSql::SurrealSql(sql) => {
                            if !sql.is_empty() {
                                // println!("{}", format!("thread {i} inserting {}.", sql.len()));
                                SUL_DB.query(sql).await.expect("insert db failed");
                                // println!("{}", format!("thread {i} finished."));
                            }
                        }
                        #[cfg(feature = "sql")]
                        SenderSql::MysqlSql((project, sql)) => {
                            let Some(pool) = pools_clone.get(&project) else { continue; };
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
            // }
        });
        all_handles.push(insert_handle);
    }

    let db_types_clone = db_types
        .into_iter()
        .map(|&x| x.to_string())
        .collect::<Vec<_>>();
    let is_sys_parse = db_types_clone.contains(&"SYST".to_string());
    let is_save_db = db_option.is_save_db();
    let parse_handle = tokio::spawn(async move {
        // let mut handles = vec![];
        //todo 按照文件大小排序，只有小于多少的能开启多线程，模型一大就不合适了
        let mut db_info_sql = vec![];
        for path in children_files {
            let file_name = path.file_name().unwrap().to_str().unwrap().to_string(); // 获取文件名

            let mut time = Instant::now();
            if is_sys_parse
                || db_option_arc.included_db_files.is_none()
                || db_option_arc
                .included_db_files
                .as_ref()
                .unwrap()
                .contains(&file_name)
            {
                let mut file = File::open(&path).await.unwrap();
                let mut buf = vec![0u8; 60];
                file.read_exact(&mut buf).await.unwrap();
                let (db_type, file_version, db_no) = parse_file_basic_info(&buf);
                let file_name_hash = hash_str(&file_name);
                db_info_sql.push(format!(
                    "INSERT IGNORE INTO db_info (id, db_type, file_version, dbnum, file_name) VALUES ('{}', '{}', '{}', '{}', '{}')",
                    file_name_hash, db_type, file_version, db_no, file_name
                ));
                if !db_types_clone.contains(&db_type) {
                    continue;
                }

                // 如果需要解析的文件列表为空或包含当前文件名，则执行以下代码块
                println!("path={:?}", &file_name); // 打印文件路径
                let project_name = project.as_str().to_string(); // 获取项目名称的字符串
                let mut db_basic = parse_file_db_basic_data(
                    &path,
                    &None,
                    &file_name,
                    project_name.clone().as_str(),
                )
                    .unwrap_or_default();
                let all_refnos = db_basic.children_map.keys().cloned().collect::<Vec<_>>();

                let db_basic = Arc::new(db_basic);
                if is_save_db {
                    save_pe_relates(&db_basic, sender.clone()).await;
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
                    dbg!(&debug_refnos);
                }

                let debug_refnos = Arc::new(debug_refnos);
                //按照SITE划分？
                for (chunk_index, chunk) in all_refnos.chunks(chunk_size).enumerate() {
                    let sender = sender.clone();
                    let chunk_refnos = chunk.to_vec();
                    let db_option_clone = db_option_arc.clone();
                    let file_name_clone = file_name.clone();
                    let chunk_refnos_clone = chunk_refnos.to_vec();
                    let project_name_clone = project_name.clone();
                    let db_basic_clone = db_basic.clone();
                    let debug_refnos = debug_refnos.clone();
                    // let handle = tokio::spawn(async move {
                    match parse_file_with_chunk(
                        db_basic_clone.clone(),
                        &None,
                        &file_name_clone,
                        project_name_clone.as_str(),
                        &chunk_refnos_clone,
                    ).await {
                        Ok(PdmsDbData {
                               total_attr_map,
                               type_ele_map,
                               db_type,
                               db_no,
                               version,
                               foreign_refnos_map,
                               ..
                           }) => {
                            //类型暂时不多线程
                            let total_attr_map_arc = Arc::new(total_attr_map);
                            //开始执行保存数据
                            // dbg!("开始保存pdms_element数据");
                            let sender_clone = sender.clone();
                            if !is_debug && is_save_db {
                                save_pes(
                                    &db_basic_clone,
                                    &total_attr_map_arc,
                                    db_no as i32,
                                    &db_option_clone,
                                    sender,
                                )
                                    .await
                                    .expect("save pe to surreal failed");
                            }
                            if b_save_mysql {
                                #[cfg(feature = "sql")]
                                    save_pes_mysql(&db_basic_clone, &project_name, &total_attr_map_arc, &pool,
                                                   &db_option_clone, db_no as i32, &sender_clone).await;
                            }
                            for kv in type_ele_map.iter() {
                                let noun: i32 = *kv.key() as _;
                                let type_name = db1_dehash(noun as _);
                                if type_name.is_empty() {
                                    continue;
                                }
                                //UDA 还是要单独存，不然数据很容易混乱
                                for refnos in &kv.value().iter().chunks(db_option_clone.att_chunk as _)
                                {
                                    let mut json_vec = vec![];
                                    let mut uda_json_vec = vec![];
                                    for refno in refnos {
                                        let att = total_attr_map_arc.get(refno).unwrap();
                                        //调试时，只解析这个单独的refno
                                        if is_debug {
                                            if debug_refnos.contains(&att.get_refno().unwrap()) {
                                                dbg!(att.value());
                                            }
                                            continue;
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
                                        uda_json_vec.push(json);
                                    }
                                    if is_save_db {
                                        if !json_vec.is_empty() {
                                            let sql = format!(
                                                "INSERT IGNORE INTO {} [{}]",
                                                &type_name,
                                                json_vec.join(",")
                                            );
                                            sender_clone.send(SurrealSql(sql)).expect("send attmap sql failed");
                                        }

                                        if !uda_json_vec.is_empty() {
                                            let sql = format!(
                                                "INSERT IGNORE INTO ATT_UDA [{}]",
                                                uda_json_vec.join(",")
                                            );
                                            sender_clone.send(SurrealSql(sql)).expect("send usa sql failed");
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

                println!("解析任务完成, 耗时: {} s", time.elapsed().as_secs_f32());
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
        let db_info_sql = db_info_sql.join(";");
        if !db_info_sql.is_empty() {
            SUL_DB.query(&db_info_sql).await.expect("save db_info failed");
        }
    });
    all_handles.push(parse_handle);
    futures::future::join_all(take(&mut all_handles)).await;
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

pub fn gen_pdms_element_insert_sql(att: &WholeAttMap, name: &str, dbno: u32, order: usize, children_count: usize) -> String {
    let implicit = &att.implicit_attmap;
    let refno = implicit.get_refno().unwrap();
    let type_name = implicit.get_type();
    let owner = implicit.get_owner();

    let mut sql = String::new();
    sql.push_str(&format!(r#"({}, '{}', '{}', {},'{}' , {} , {} , {} ,0 ) ,"#,
                          refno.0, refno.to_pdms_str(), type_name, owner.0, name, dbno, order, children_count));
    sql
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
