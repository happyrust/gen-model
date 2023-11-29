use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::fs::File;
use std::mem::take;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use aios_core::consts::*;
use aios_core::orm::*;
use aios_core::pdms_types::*;
use aios_core::tool::db_tool::{db1_dehash, db1_hash};
use aios_core::tool::float_tool::f64_round_3;
use aios_core::types::*;
use dashmap::{DashMap, DashSet};
use itertools::Itertools;
use parse_pdms_db::parse::*;
use sea_orm::{ConnectionTrait, Schema, Statement};
use sqlx::pool::PoolConnection;
use sqlx::{Connection, MySql, MySqlPool, Pool};
use sqlx::{Error, Executor};

use crate::api::element::*;
use crate::aql_api::PdmsPLINAttrAql;
use crate::arangodb::{ArDatabase, ArPool};
use crate::consts::*;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::*;
use crate::graph_db::ParaDocument;
use crate::tables;
use crate::tables::*;
use crate::versioned_db::client::*;
use aios_core::cache::mgr::BytesTrait;
use aios_core::helper::table::{qualified_column_name, qualified_table_name};
use aios_core::options::DbOption;
use aios_core::pdms_data::ATTR_INFO_MAP;
use aios_core::AttrVal::StringType;
use aios_core::SUL_DB;
use aios_core::{get_default_pdms_db_info, orm};
use bevy_reflect::DynamicStruct;
use parry3d::utils::hashmap::FxHasher32;
use std::hash::{Hash, Hasher};
use std::io::Read;

pub trait MySqlMethods {
    fn add_to_args(&self, args: &mut sqlx::mysql::MySqlArguments);

    fn get_query(count: usize) -> anyhow::Result<String>;

    fn name() -> String;
}

/// 初始化project database
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
pub async fn create_info_database(url: &str, project_name: &str) -> anyhow::Result<()> {
    let pool = MySqlPool::connect(&url).await?;
    pool.execute(
        format!(
            "CREATE DATABASE IF NOT EXISTS {PDMS_INFO_DB}_{};",
            project_name
        )
        .as_str(),
    )
    .await?;

    //todo 改成一对多的实现
    let mut pool =
        AiosDBManager::get_db_pool(&url, &format!("{}_{}", PDMS_INFO_DB, project_name)).await?;
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
        .execute(gen_create_version_info_table_sql(project_name).as_str())
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
    let mut time = Instant::now();
    // 获取默认的数据库连接字符串
    let default_conn_str = AiosDBManager::get_default_conn_str(db_option);
    if db_option.sync_tidb.unwrap_or(false) {
        create_info_database(&default_conn_str, &db_option.project_name).await?;
    }
    //针对一些特殊的表，需要先创建表，定义索引
    // SUL_DB
    //     .query(
    //         r#"
    // DEFINE INDEX unique_pe_owner
    // ON TABLE pe_owner
    // COLUMNS in, out UNIQUE;
    // "#,
    //     )
    //     .await
    //     .unwrap();
    let mut create_tables_elapse = 0;
    // 执行多线程解析
    dbg!("执行多线程解析");
    // 遍历所有包含的项目
    for project in &db_option.included_projects {
        if db_option.sync_tidb.unwrap_or(false) {
            let db = sea_orm::Database::connect(&default_conn_str).await.unwrap();
            let backend = db.get_database_backend();
            let schema = Schema::new(backend);
            db.execute(Statement::from_string(
                backend.clone(),
                format!("CREATE DATABASE IF NOT EXISTS {project} DEFAULT CHARSET UTF8;"),
            ))
            .await?;
            let project_db = sea_orm::Database::connect(&db_option.get_mysql_project_db_conn_str())
                .await
                .unwrap();
            let mut create_table_sqls = orm::sql::get_all_create_table_sqls().unwrap();
            for x in create_table_sqls {
                project_db.execute_unprepared(&x).await.unwrap();
            }
        }

        match sync_total_async_threaded(&db_option, project, &["DICT", "SYST"], false).await {
            Ok(_) => {
                // 同步数据成功
                println!("同步UDA和SYS数据成功。");
            }
            Err(e) => {
                // 同步数据失败，打印错误信息
                println!("{}", e.to_string());
            }
        }

        match sync_total_async_threaded(&db_option, project, &["DESI", "CATA"], true).await {
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

#[inline]
pub fn gen_uda_attr_value_sql(att: &WholeAttMap) -> String {
    let mut table_vals_sql = String::new();
    // let i_att = &att.implicit_attmap;
    // let refno = i_att.get_refno().unwrap(); // 获取引用号
    // let type_name = i_att.get_type_str(); // 获取类型名称
    // let owner = i_att.get_owner(); // 获取所有者
    // let data = hex::encode(att.uda_attmap.into_compress_bytes()); // 将uda_attmap转换为压缩字节并进行十六进制编码
    // table_vals_sql.push_str(&format!(
    //     r#"({}, '{}', '{}', {}, 0x{}),"#, // 插入语句模板
    //     refno.0,                          // 引用号的第一个元素
    //     refno.to_refno_str(),             // 引用号的字符串表示
    //     type_name,                        // 类型名称
    //     owner.0,                          // 所有者的第一个元素
    //     data                              // 数据
    // ));
    table_vals_sql
}

//分成两部分，一部分先保存UDA 和 SYS 这些数据
///多线程同步数据，包括增量同步
pub async fn sync_total_async_threaded(
    db_option: &DbOption,
    project: &str,
    db_types: &[&str],
    debug_need: bool,
) -> anyhow::Result<()> {
    let mut data_dir = Path::new(&db_option.project_path); // 创建一个Path对象，表示数据目录的路径
    let need_parsed_files = &db_option.included_db_files; // 获取需要解析的数据库文件列表
    let project_dir = data_dir.join(&project); // 创建一个Path对象，表示项目目录的路径
    let max_sql_threads_number = db_option.sql_threads_number as usize; // 获取最大SQL线程数
    let batch_insert_sql_cnt = db_option.batch_insert_sql_cnt as usize; // 获取批量插入SQL数量
    if max_sql_threads_number * batch_insert_sql_cnt == 0 {
        // 如果最大SQL线程数和批量插入SQL数量之积为0，则抛出错误
        return Err(anyhow::anyhow!(
            "batch_insert_sql_cnt 或者  sql_threads_number 不能为0"
        ));
    }
    if !Path::new(&project_dir).exists() {
        // 如果项目目录不存在，则抛出错误
        return Err(anyhow::anyhow!("项目文件夹指定不正确"));
    }
    let mut children_files = {
        // 获取子文件列表
        let target_dir = fs::read_dir(&project_dir)
            .unwrap()
            .into_iter()
            .map(|entry| {
                let entry = entry.unwrap();
                entry.path()
            })
            .find(|x| x.is_dir() && x.file_name().unwrap().to_str().unwrap().ends_with("000"))
            .unwrap();
        fs::read_dir(target_dir)?
            .into_iter()
            .map(|entry| {
                let entry = entry.unwrap();
                entry.path()
            })
            .collect::<Vec<PathBuf>>()
    };
    // 先解析一遍uda
    // 正式解析
    let project = Arc::new(project.to_string()); // 创建一个Arc对象，表示项目名称
    let db_option = Arc::new(db_option.clone()); // 创建一个Arc对象，表示数据库选项
    let mut is_replace = db_option.replace_dbs; // 是否替换数据库的数据
    let replace_types = db_option.replace_types.clone(); // 获取替换的类型列表
    let b_replace_types = replace_types.is_some(); // 是否存在替换的类型列表
    if b_replace_types {
        is_replace = true;
    }
    let mut uda_map: HashMap<i32, AttrMap> = HashMap::new();
    // let mut version_map = HashMap::new();
    let only_update_dbinfo = db_option.only_update_dbinfo;
    let only_sync_sys = db_option.only_sync_sys;
    let chunk_size = db_option.sync_chunk_size.unwrap_or(10_0000) as usize;

    let sync_tidb = db_option.sync_tidb.unwrap_or(false);

    for path in children_files {
        let file_name = path.file_name().unwrap().to_str().unwrap().to_string(); // 获取文件名
        {
            let mut file = File::open(&path).unwrap();
            let mut buf = vec![0u8; 60];
            file.read_exact(&mut buf)?;
            let (db_type, file_version, db_no) = parse_file_basic_info(&buf);
            if !db_types.contains(&db_type.as_str()) {
                continue;
            }
            // dbg!(&(db_type.as_str(), file_version, db_no, &file_name));
        }

        if !debug_need
            || need_parsed_files.is_none()
            || need_parsed_files.as_ref().unwrap().contains(&file_name)
        {
            // 如果需要解析的文件列表为空或包含当前文件名，则执行以下代码块
            println!("path={:?}", &file_name); // 打印文件路径
            let project_clone = project.clone(); // 创建项目名称的克隆
            let project_name = project.as_str().to_string(); // 获取项目名称的字符串
            let mut children_map =
                parse_file_children_map(&path, &None, &file_name, project_name.clone().as_str())
                    .unwrap_or_default();
            dbg!(children_map.len());
            let all_refnos = children_map.keys().cloned().collect::<Vec<_>>();
            let children_map_clone = Arc::new(children_map);

            if db_option.sync_graph_db.unwrap_or(true) {
                let arango_pool = connect_arangodb(&db_option).await?;
                let database = arango_pool
                    .get()
                    .await?
                    .db(&db_option.arangodb_database)
                    .await?; // 获取ArangoDB数据库连接
                save_pdms_level_edges_in_sync(&database, &children_map_clone).await?;
            }

            for (chunk_index, chunk_refnos) in all_refnos.chunks(chunk_size).enumerate() {
                let path_clone = path.clone();
                let file_name_clone = file_name.clone();
                let chunk_refnos_clone = chunk_refnos.to_vec();
                let project_name_clone = project_name.clone();
                if let Ok(PdmsDbData {
                    total_attr_map,
                    type_ele_map,
                    refno_info_map,
                    db_type,
                    db_no,
                    version,
                    foreign_refnos_map,
                    ..
                }) = parse_file_with_chunk(
                    &path_clone,
                    &None,
                    &file_name_clone,
                    project_name_clone.as_str(),
                    &chunk_refnos_clone,
                )
                .await
                {
                    println!("Processing {} chunk index: {chunk_index}", &file_name);

                    //类型暂时不多线程
                    let total_attr_map_arc = Arc::new(total_attr_map);
                    let children_map_arc = children_map_clone.clone();
                    // 将部分数据保存到图数据库
                    if db_option.sync_graph_db.unwrap_or(true) {
                        //if db_type == "CATA" || db_type == "DESI"
                        let arango_pool = connect_arangodb(&db_option).await?;
                        {
                            let database = arango_pool
                                .get()
                                .await?
                                .db(&db_option.arangodb_database)
                                .await?;
                            // 将 pdms_element 部分数据保存到图数据库中
                            save_pdms_element_to_arango(
                                &database,
                                &total_attr_map_arc,
                                &children_map_arc,
                                db_no as i32,
                            )
                            .await?;
                            save_foreign_refno_edges_in_sync(&database, foreign_refnos_map).await?;
                        }
                        println!("图数据库保存完成");
                    }

                    //开始执行保存数据
                    dbg!("开始保存pdms_element数据");
                    save_pdms_eles_to_surreal(
                        &total_attr_map_arc,
                        db_no as i32,
                        &children_map_clone,
                    )
                    .await?;
                    dbg!("开始保存属性数据");
                    const ATTS_CHUNK_COUNT: usize = 300;
                    let mut join_set = tokio::task::JoinSet::new();
                    let mut save_atts_time = Instant::now();
                    for kv in type_ele_map.iter() {
                        let noun: i32 = *kv.key() as _;
                        let type_name = db1_dehash(noun as _);
                        if type_name.is_empty() {
                            continue;
                        }
                        for refnos in &kv.value().iter().chunks(ATTS_CHUNK_COUNT) {
                            let mut json_vec = vec![];
                            for refno in refnos {
                                let att = total_attr_map_arc.get(refno).unwrap();
                                let Some(json) = att.gen_sur_json() else {
                                    continue;
                                };
                                json_vec.push(json);
                            }
                            let sql = format!(
                                "INSERT IGNORE INTO {} [{}]",
                                &type_name,
                                json_vec.join(",")
                            );
                            //使用surreal 保存NamedAttrMap
                            join_set.spawn(async move {
                                SUL_DB.query(sql).await.unwrap();
                            });
                        }
                    }
                    //等待保存任务完成
                    while let Some(_) = join_set.join_next().await {}
                    println!(
                        "保存属性数据完成，耗时: {} s",
                        save_atts_time.elapsed().as_secs_f32()
                    );
                }
            }
        }

        //重新更新一下database info，有可能发生了更新
        let db_info = get_default_pdms_db_info();
        let _ = db_info.save(None);
    }
    if sync_tidb {
        let default_conn_str = AiosDBManager::get_default_conn_str(&db_option);
        let pool = AiosDBManager::get_db_pool(&default_conn_str, project.as_str()).await?;
        // 保存 uda_map
        if uda_map.len() > 0 {
            let mut uda_sql = format!("INSERT IGNORE INTO {PDMS_UDA_ATT_TABLE} (TYPE,DATA) VALUES");
            for (noun, value) in uda_map.into_iter() {
                let data = value.into_compress_bytes();
                uda_sql.push_str(&format!("({},0x{}),", noun, hex::encode(data)))
            }
            let mut project_conn = pool.acquire().await.unwrap();
            uda_sql.remove(uda_sql.len() - 1);
            let result = project_conn.execute(uda_sql.as_str()).await;
            match result {
                Ok(_) => {}
                Err(e) => {
                    dbg!(&e);
                    dbg!(uda_sql.as_str());
                }
            }
        }
    }

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
