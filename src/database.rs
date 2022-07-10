use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::format;
use std::time::Instant;
use aios_core::pdms_types::{AttrMap, AttrVal, NounHash, PdmsDatabaseInfo, RefU64, RefU64Vec};
use aios_core::pdms_types::AttrVal::StringType;
use aios_core::tool::db_tool::{db1_dehash, db1_hash};
use sqlx::{MySql, MySqlPool, Pool};
use sqlx::mysql::MySqlArguments;
use sqlx::pool::PoolConnection;
use parse_pdms_db::parse::{PdmsDbData, WholeAttMap};
use parse_pdms_db::tool::hash_tool::f64_round_3;
use std::path::{Path, PathBuf};
use std::fs;
use std::sync::Arc;
use parse_pdms_db::parse_file;
use std::mem::take;
use aios_core::consts::*;
use dashmap::DashMap;
use itertools::Itertools;
use crate::api::project_mdb::insert_project_mdb;
use crate::consts::*;
use crate::{ATTR_INFO_MAP, options, tables};
use crate::api::element::*;
use crate::helper::{qualified_column_name, qualified_table_name};
use crate::options::DbOption;
use sqlx::Executor;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::ssc::{gen_insert_ssc_node_sql, insert_set_ssc_node_sql, insert_ssc_room_node};
use crate::tables::{gen_creat_version_info_table_sql, gen_create_data_state_tables_sql, gen_create_pdms_version_table_sql, gen_create_project_mdb_json_sql, gen_create_room_code_table_sql};

pub trait MySqlMethods {
    fn add_to_args(&self, args: &mut sqlx::mysql::MySqlArguments);

    fn get_query(count: usize) -> anyhow::Result<String>;

    fn name() -> String;
}


//创建database
pub async fn init_database(project: &str, url: &str) -> anyhow::Result<()> {
    let connection = MySqlPool::connect(url)
        .await
        .unwrap();
    let mut pool = connection.try_acquire().unwrap();

    // sqlx::query(&format!("DROP DATABASE IF EXISTS {project}")).execute(&mut pool).await?;
    sqlx::query(&format!("CREATE DATABASE IF NOT EXISTS {project} DEFAULT CHARSET UTF8")).execute(&mut pool).await?;
    Ok(())
}

/// 创建 info 库和表
pub async fn init_info_database(url: &str, project_name: &str) -> anyhow::Result<()> {
    let pool = MySqlPool::connect(&url).await?;
    pool.execute(format!("CREATE DATABASE IF NOT EXISTS {PDMS_INFO_DB}_{};", project_name).as_str()).await?;

    let mut pool = AiosDBManager::get_db_pool(&url, &format!("{}_{}", PDMS_INFO_DB, project_name)).await?;
    let mut sql = String::new();
    sql.push_str(&format!(r#"CREATE TABLE IF NOT EXISTS {} ("#, { PDMS_REFNO_INFOS_TABLE }));
    sql.push_str(&format!(r#"{} BIGINT NOT NULL PRIMARY KEY ,"#, "REF0"));
    sql.push_str(&format!(r#"{} VARCHAR(20)"#, "PROJECT"));

    sql.push_str(");");
    let result = pool.execute(sql.as_str()).await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(e);
            dbg!(sql.as_str());
        }
    }

    Ok(())
}


/// 同步pdms数据到数据
pub async fn sync_pdms(db_option: &DbOption) -> anyhow::Result<()> {
    println!("开始同步pdms/E3D: {} 的数据", &db_option.project_name);
    let mut time = Instant::now();
    let default_conn_str = AiosDBManager::get_default_conn_str(db_option);
    init_info_database(&default_conn_str, &db_option.project_name).await?;
    let pdms_info_pool = AiosDBManager::get_db_pool(&default_conn_str, &format!("{}_{}", PDMS_INFO_DB, &db_option.project_name)).await?;
    let mut pdms_info_conn = pdms_info_pool.clone().acquire().await?;
    let mut create_tables_elapse = 0;
    for project in &db_option.included_projects {
        if db_option.recreate_db {
            init_database(project, &default_conn_str).await?;
            let mut table_time = Instant::now();
            let mut tables_sql = String::new();
            if !db_option.only_rebuild_pdms_element {
                if let Ok(db_info) = serde_json::from_str::<PdmsDatabaseInfo>(&include_str!("../all_attr_info.json")) {
                    for (k, v) in db_info.noun_attr_info_map {
                        let mut attr_map = BTreeMap::new();
                        let type_name = db1_dehash(k as u32);
                        if type_name.is_empty() {
                            continue;
                        }
                        let mut tmp_sets = HashSet::new();
                        for (kk, vv) in v {
                            let att_name = vv.name.to_string();
                            if &att_name != "unset" {
                                if att_name.starts_with(":") || vv.offset == 0 {
                                    continue;
                                }
                                if !tmp_sets.contains(&att_name) {
                                    tmp_sets.insert(att_name.clone());
                                } else {
                                    continue;
                                }
                                if kk == *TYPE_HASH as i32 {
                                    attr_map.insert(vv.offset, (att_name, StringType(db1_dehash(k as u32).into())));
                                } else {
                                    attr_map.insert(vv.offset, (att_name, vv.default_val));
                                }
                            }
                        }
                        tables_sql.push_str(&tables::gen_create_implicit_tables_sql(type_name.as_str(), &attr_map));
                        tables_sql.push_str(&tables::gen_create_explicit_tables_sql());
                        tables_sql.push_str(&tables::gen_create_uda_tables_sql());
                    }
                }
            }

            let project_pool = AiosDBManager::get_db_pool(&default_conn_str, project).await?;
            let mut conn = project_pool.acquire().await?;
            tables_sql.push_str(&tables::gen_create_element_tables_sql(db_option.only_rebuild_pdms_element));
            if !db_option.only_rebuild_pdms_element {
                tables_sql.push_str(&tables::gen_create_project_mdb_sql());
                tables_sql.push_str(&gen_create_project_mdb_json_sql());
                tables_sql.push_str(&gen_create_data_state_tables_sql());
                tables_sql.push_str(&gen_create_pdms_version_table_sql());
                tables_sql.push_str(&gen_create_room_code_table_sql());
            }
            let result = conn.execute(tables_sql.as_str()).await;
            match result {
                Ok(_) => {}
                Err(e) => {
                    dbg!(&e);
                    dbg!(tables_sql.as_str());
                }
            }


            create_tables_elapse += table_time.elapsed().as_millis();
            let result = pdms_info_conn.execute(tables::gen_create_dbno_infos_tables_sql().as_str()).await;
            match result {
                Ok(_) => {}
                Err(e) => {
                    dbg!(&e);
                    dbg!(tables_sql.as_str());
                }
            }
            let result = pdms_info_conn.execute(gen_creat_version_info_table_sql(&db_option.project_name).as_str()).await;
            match result {
                Ok(_) => {}
                Err(e) => {
                    dbg!(&e);
                    dbg!(tables_sql.as_str());
                }
            }
        }
        let project_pool = AiosDBManager::get_db_pool(&default_conn_str, project).await?;
        if db_option.types_multi_thread {
            dbg!("执行多线程解析");
            sync_total_async_threading(&db_option, project, project_pool.clone(),
                                       pdms_info_pool.clone()).await.expect("同步数据失败");
        } else {
            dbg!("执行单线程解析");
            sync_total_async(&db_option, project, project_pool.clone(),
                             pdms_info_pool.clone()).await.expect("同步数据失败");
        }
        if !db_option.only_rebuild_pdms_element {
            insert_project_mdb(&project_pool, &pdms_info_pool).await?;
        }
    }

    println!("创建表花费时间: {} ms", create_tables_elapse);
    println!("初始化数据库时间: {} ms", time.elapsed().as_millis() - create_tables_elapse);

    Ok(())
}


pub fn gen_explicit_att_insert_sql(refno: RefU64, type_name: &str, owner: RefU64, e_att: &AttrMap) -> String {
    let mut sql = String::new();
    let mut table_columns_sql = String::new();
    let table_name = qualified_table_name(type_name);
    table_columns_sql.push_str("REPLACE INTO {PDMS_EXPLICIT_TABLE} (ID, REFNO, TYPE, OWNER, DATA)");

    let mut table_vals_sql = String::new();
    let data = hex::encode(e_att.into_compress_bytes());
    table_vals_sql.push_str(&format!(r#"({}, '{}', '{}', {}, 0x{})"#, refno.0, refno.to_refno_str(), table_name, owner.0, data));


    sql.push_str(&table_columns_sql);
    sql.push_str(" VALUES ");
    sql.push_str(&table_vals_sql);

    sql
}

/// 生成隐藏属性的插入语句的前面列名部分
pub fn gen_implicit_attr_insert_sql(hash: u32) -> (String, Vec<NounHash>) {
    // let i_att = &att.implicit_attmap;
    let type_name = db1_dehash(hash);
    let table_name = qualified_table_name(type_name.as_str());
    let mut table_columns_sql = String::new();
    table_columns_sql.push_str(&format!("REPLACE INTO {} (ID, REFNO, TYPE, OWNER", table_name));

    let implicit_names = ATTR_INFO_MAP.get_type_implicit_att_names(type_name.as_str());
    let column_hashs = implicit_names.iter().filter_map(|x| (x != "unset").then(|| NounHash(db1_hash(x.as_str())))).collect();
    let v_sql = implicit_names.iter().map(|x| qualified_column_name(x.as_str()))
        .join(",");
    // dbg!(&v_sql);
    if v_sql.len() > 0 {
        table_columns_sql.push_str(" , ");
    }
    table_columns_sql.push_str(v_sql.as_str());
    // for (k, v) in &i_att.map {
    //     let mut att_name_full = db1_dehash(k.0);
    //     let att_name = qualified_column_name(att_name_full.as_str());
    //     if att_name.starts_with(":") || att_name.as_str() == "REFNO" || att_name.as_str() == "TYPE" || att_name.as_str() == "OWNER" {
    //         continue;
    //     }
    //     match v {
    //         AttrVal::InvalidType => {}
    //         _ => {
    //             table_columns_sql.push_str(&format!("{},", att_name.as_str()));
    //             column_hashs.push(k.clone());
    //         }
    //     }
    // }
    // table_columns_sql.remove(table_columns_sql.len() - 1);
    table_columns_sql.push_str(") VALUES ");

    (table_columns_sql, column_hashs)
}

#[inline]
pub fn gen_explicit_attr_value_sql(att: &WholeAttMap) -> String {
    let mut table_vals_sql = String::new();
    let i_att = &att.implicit_attmap;
    let refno = i_att.get_refno().unwrap();
    let type_name = i_att.get_type();
    let owner = i_att.get_owner().unwrap();
    let data = hex::encode(att.explicit_attmap.into_compress_bytes());
    table_vals_sql.push_str(&format!(r#"({}, '{}', '{}', {}, 0x{}),"#, refno.0, refno.to_refno_str(), type_name, owner.0, data));

    table_vals_sql
}

/// 生成隐藏属性的插入语句的后面数据部分
pub fn gen_implicit_attr_value_sql(att: &WholeAttMap, column_hashes: &Vec<NounHash>) -> String {
    let mut table_vals_sql = String::new();
    let i_att = &att.implicit_attmap;
    let refno = i_att.get_refno().unwrap();
    let type_name = i_att.get_type();
    let owner = i_att.get_owner().unwrap();
    table_vals_sql.push_str(&format!(r#"({}, '{}', '{}', {},"#, refno.0, refno.to_refno_str(), type_name, owner.0));
    for noun_hash in column_hashes {
        if let Some(v) = i_att.get(noun_hash) {
            if noun_hash != &NounHash(UNSET_NOUN) {
                match v {
                    AttrVal::InvalidType => {}
                    AttrVal::IntegerType(d) => {
                        table_vals_sql.push_str(&format!("{},", d.to_string()));
                    }
                    AttrVal::StringType(d) => {
                        table_vals_sql.push_str(&format!(r#"'{}',"#, d.replace(r#"'"#, "")));
                    }
                    AttrVal::DoubleType(d) => {
                        table_vals_sql.push_str(&format!("{},", f64_round_3(*d)));
                    }
                    AttrVal::DoubleArrayType(d) => {
                        table_vals_sql.push_str(&format!(r#"0x{},"#, hex::encode(bincode::serialize(d).unwrap_or_default().as_slice())));
                    }
                    AttrVal::StringArrayType(d) => {
                        table_vals_sql.push_str(&format!(r#"'{}',"#, serde_json::to_string(d).unwrap_or_default()));
                    }
                    AttrVal::BoolArrayType(d) => {
                        table_vals_sql.push_str(&format!(r#"'{}',"#, serde_json::to_string(d).unwrap_or_default()));
                    }
                    AttrVal::IntArrayType(d) => {
                        table_vals_sql.push_str(&format!(r#"'{}',"#, serde_json::to_string(d).unwrap_or_default()));
                    }
                    AttrVal::BoolType(d) => {
                        let b = if *d { 1 } else { 0 };
                        table_vals_sql.push_str(&format!("{},", b));
                    }
                    AttrVal::Vec3Type(d) => {
                        table_vals_sql.push_str(&format!(r#"'{}',"#, serde_json::to_string(d).unwrap_or_default()));
                    }
                    AttrVal::ElementType(d) => {
                        table_vals_sql.push_str(&format!(r#"'{}',"#, d));
                    }
                    AttrVal::WordType(d) => {
                        table_vals_sql.push_str(&format!(r#"'{}',"#, d));
                    }
                    AttrVal::RefU64Type(d) => {
                        table_vals_sql.push_str(&format!("{},", d.0));
                    }
                    AttrVal::RefU64Array(d) => {
                        table_vals_sql.push_str(&format!(r#"'{}',"#, serde_json::to_string(d).unwrap_or_default()));
                    }
                    AttrVal::StringHashType(_) => {}
                }
            }
        }
    }


    table_vals_sql.remove(table_vals_sql.len() - 1);
    table_vals_sql.push_str("),");

    table_vals_sql
}

///多线程保存
pub async fn sync_total_async(db_option: &options::DbOption, project: &str,
                              pool: Pool<MySql>, info_pool: Pool<MySql>) -> anyhow::Result<()> {
    let mut data_dir = Path::new(&db_option.project_path);
    let need_parsing_files = &db_option.included_db_files;
    let project_dir = data_dir.join(&project);
    let batch_chunks_cnt = db_option.sql_batch_insert_chunk as usize;
    let _batch_handles_cnt = db_option.batch_insert_handles_chunk as usize;
    let mut target_dir = fs::read_dir(&project_dir).unwrap().into_iter().map(|entry| {
        let entry = entry.unwrap();
        entry.path()
    }).find(|x| x.file_name().unwrap().to_str().unwrap().ends_with("000")).unwrap();

    let mut children_files = fs::read_dir(target_dir)?.into_iter().map(|entry| {
        let entry = entry.unwrap();
        entry.path()
    }).collect::<Vec<PathBuf>>();

    let mut handles = vec![];
    let project = Arc::new(project.to_string());
    let db_option = Arc::new(db_option.clone());
    for path in children_files {
        let file_name = path.file_name().unwrap().to_str().unwrap().to_string();
        let file_name_clone = Arc::new(file_name.clone());
        if !file_name.ends_with("com") && !file_name.ends_with("mis") {
            if need_parsing_files.is_none() || need_parsing_files.as_ref().unwrap().contains(&file_name) {
                println!("path={:?}", &file_name);
                let project = project.clone();
                let project_clone = project.to_string();
                let pool_clone = pool.clone();
                let info_pool_clone = info_pool.clone();
                let filename_clone = file_name_clone.clone();
                let db_option_clone = db_option.clone();
                let handle = tokio::spawn(async move {
                    let project_clones = project_clone.clone();
                    //后面再考虑成不同的table，如显示属性和隐藏属性
                    if let Ok(Ok(PdmsDbData {
                                     all_attr_map,
                                     total_attr_map,
                                     ele_id_tree,
                                     type_ele_map,
                                     refno_node_id_map,
                                     refno_info_map,
                                     children_map,
                                     db_type,
                                     db_no,
                                     field_no,
                                     version,
                                     room_code_map,
                                     ..
                                 })) = tokio::task::spawn_blocking(move || {
                        parse_file(&path, &None, &file_name, &project_clone.clone(), "")
                    }).await {
                        for kv in &type_ele_map {
                            let type_hash = *kv.key();
                            let mut implicit_insert_data_sql = None;
                            let mut ref0_info_sql = String::new();
                            let mut implicit_values_sql = String::new();
                            let mut explicit_values_sql = String::new();
                            let mut pdms_elements_sql = String::new();
                            let mut dbno_filename_sql = gen_dbno_filename_insert_sql(db_no.0, &filename_clone.clone(),
                                                                                     version.0, &project_clones, db_type.clone());
                            let mut info_conn = info_pool_clone.acquire().await.unwrap();
                            //保存dbno的信息表
                            if !db_option_clone.only_rebuild_pdms_element {
                                let mut sql = format!("REPLACE INTO {PDMS_DBNO_INFOS_TABLE} ( NUMBDB,FILENAME,VERSION,PROJECT,DB_TYPE ) VALUES ");
                                sql.push_str(dbno_filename_sql.as_str());
                                // sql.remove(sql.len() - 1);
                                let result = info_conn.execute(sql.as_str()).await;
                                match result {
                                    Ok(_) => {}
                                    Err(e) => {
                                        dbg!(&e);
                                        dbg!(sql.as_str());
                                    }
                                }
                                //保存refno的信息表
                                let mut sql = format!("REPLACE INTO {PDMS_REFNO_INFOS_TABLE}(REF0, PROJECT) VALUES ");
                                for kv in &refno_info_map {
                                    sql.push_str(&format!(r#"({},'{}') ,"#, kv.value().ref_0, /*v.db_no, */project.as_str()));
                                }
                                sql.remove(sql.len() - 1);
                                let result = info_conn.execute(sql.as_str()).await;
                                match result {
                                    Ok(_) => {}
                                    Err(e) => {
                                        dbg!(&e);
                                        dbg!(sql.as_str());
                                    }
                                }
                            }

                            for (i, refno) in kv.value().iter().enumerate() {
                                let att = total_attr_map.get(refno).unwrap();
                                if implicit_insert_data_sql.is_none() {
                                    implicit_insert_data_sql = Some(gen_implicit_attr_insert_sql(type_hash));
                                }
                                let columns = &implicit_insert_data_sql.as_ref().unwrap().1;
                                // ref0_info_sql.push_str(&gen_refno_infos_insert_sql(*refno, &project_clones.clone()));
                                implicit_values_sql.push_str(&gen_implicit_attr_value_sql(att.value(), columns));
                                explicit_values_sql.push_str(&gen_explicit_attr_value_sql(att.value()));
                                let name = get_name(&total_attr_map, &children_map, *refno).replace(r#"'"#, r#"\'"#)
                                    .replace(r#"""#, r#"\""#);
                                let order = get_order(&total_attr_map, &children_map, *refno);
                                let children_count = children_map.get(&refno).unwrap_or(&RefU64Vec::default()).len();
                                pdms_elements_sql.push_str(&gen_pdms_element_insert_sql(att.value(), &name, db_no.0, order, children_count));
                                //获取当前项目的连接
                                let mut project_conn = pool_clone.acquire().await.unwrap();

                                // let mut insert_join_handles = vec![];
                                if (i != 0 && i % batch_chunks_cnt == 0) || i == (kv.value().len() - 1) {
                                    let info_sql = take(&mut ref0_info_sql);
                                    let implicit_values_sql = take(&mut implicit_values_sql);
                                    let explicit_values_sql = take(&mut explicit_values_sql);
                                    let pdms_elements_sql = take(&mut pdms_elements_sql);
                                    // let implicit_query_data = implicit_insert_data_sql.clone();
                                    if !db_option_clone.only_rebuild_pdms_element {
                                        //执行隐式数据保存
                                        let mut sql = String::new();
                                        sql.push_str(implicit_insert_data_sql.as_ref().unwrap().0.as_str());
                                        sql.push_str(implicit_values_sql.as_str());
                                        sql.remove(sql.len() - 1);
                                        let result = project_conn.execute(sql.as_str()).await;
                                        match result {
                                            Ok(_) => {}
                                            Err(_) => {
                                                dbg!(sql.as_str());
                                            }
                                        }
                                        //执行显示数据保存
                                        let mut sql = format!("REPLACE INTO {PDMS_EXPLICIT_TABLE} (ID, REFNO, TYPE, OWNER, DATA) VALUES ");
                                        sql.push_str(explicit_values_sql.as_str());
                                        sql.remove(sql.len() - 1);
                                        let result = project_conn.execute(sql.as_str()).await;
                                        match result {
                                            Ok(_) => {}
                                            Err(e) => {
                                                dbg!(&e);
                                                dbg!(sql.as_str());
                                            }
                                        }
                                    }
                                    // {PDMS_ELEMENTS_TABLE} 保存
                                    let mut sql = format!("REPLACE INTO {PDMS_ELEMENTS_TABLE} (ID, REFNO, TYPE, OWNER, NAME, NUMBDB , ORDER_NUM, IS_DEL ) VALUES ");
                                    sql.push_str(pdms_elements_sql.as_str());
                                    sql.remove(sql.len() - 1);
                                    let result = project_conn.execute(sql.as_str()).await;
                                    match result {
                                        Ok(_) => {}
                                        Err(e) => {
                                            dbg!(&e);
                                            dbg!(sql.as_str());
                                        }
                                    }
                                }
                            }
                        }
                        let mut project_conn = pool_clone.acquire().await.unwrap();
                        if !db_option_clone.only_rebuild_pdms_element {
                            // 将带有 room_code 属性的保存下来
                            for (room_name, refnos) in room_code_map.clone() {
                                // 将 room_code 单独存放到 room_code 表中
                                let mut room_code_sql = format!("REPLACE INTO {ROOM_CODE} (REFNO,ROOM_NAME) VALUES ");
                                for refno in refnos.clone() {
                                    room_code_sql.push_str(&format!("( {},'{}' ) ,", refno.0, room_name.clone()));
                                }
                                room_code_sql.remove(room_code_sql.len() - 1);
                                let result = project_conn.execute(room_code_sql.as_str()).await;
                                match result {
                                    Ok(_) => {}
                                    Err(e) => {
                                        dbg!(&e);
                                        dbg!(room_code_sql.as_str());
                                    }
                                }
                            }
                        }
                    }
                });
                handles.push(handle);
            }
        }
        // break;
    }

    futures::future::join_all(handles).await;

    Ok(())
}


///单线程保存
pub async fn sync_total_async_threading(db_option: &options::DbOption, project: &str, pool: Pool<MySql>, info_pool: Pool<MySql>) -> anyhow::Result<()> {
    let mut data_dir = Path::new(&db_option.project_path);
    let need_parsing_files = &db_option.included_db_files;
    let project_dir = data_dir.join(&project);
    let batch_chunks_cnt = db_option.sql_batch_insert_chunk as usize;
    let batch_handles_cnt = db_option.batch_insert_handles_chunk as usize;
    let mut target_dir = fs::read_dir(&project_dir).unwrap().into_iter().map(|entry| {
        let entry = entry.unwrap();
        entry.path()
    }).find(|x| x.file_name().unwrap().to_str().unwrap().ends_with("000")).unwrap();

    let mut children_files = fs::read_dir(target_dir)?.into_iter().map(|entry| {
        let entry = entry.unwrap();
        entry.path()
    }).collect::<Vec<PathBuf>>();

    // let mut handles = vec![];
    let project = Arc::new(project.to_string());
    let db_option = Arc::new(db_option.clone());
    for path in children_files {
        let file_name = path.file_name().unwrap().to_str().unwrap().to_string();
        let file_name_clone = Arc::new(file_name.clone());
        if !file_name.ends_with("com") && !file_name.ends_with("mis") {
            if need_parsing_files.is_none() || need_parsing_files.as_ref().unwrap().contains(&file_name) {
                println!("path={:?}", &file_name);
                let project = project.clone();
                let project_clone = project.to_string();
                let pool_clone = pool.clone();
                let info_pool_clone = info_pool.clone();
                let filename_clone = file_name_clone.clone();
                let db_option_clone = db_option.clone();
                // let handle = tokio::spawn(async move {
                let project_clones = project_clone.clone();
                //后面再考虑成不同的table，如显示属性和隐藏属性
                if let Ok(Ok(PdmsDbData {
                                 all_attr_map,
                                 total_attr_map,
                                 ele_id_tree,
                                 type_ele_map,
                                 refno_node_id_map,
                                 refno_info_map,
                                 children_map,
                                 db_type,
                                 db_no,
                                 field_no,
                                 version,
                                 room_code_map,
                                 ..
                             })) = tokio::task::spawn_blocking(move || {
                    parse_file(&path, &None, &file_name, &project_clone.clone(), "")
                }).await {
                    for kv in &type_ele_map {
                        let type_hash = *kv.key();
                        let mut implicit_query_data = None;
                        let mut ref0_info_sql = String::new();
                        let mut implicit_values_sql = String::new();
                        let mut explicit_values_sql = String::new();
                        let mut pdms_elements_sql = String::new();
                        let mut dbno_filename_sql = gen_dbno_filename_insert_sql(db_no.0, &filename_clone.clone(),
                                                                                 version.0, &project_clones, db_type.clone());
                        let mut info_conn = info_pool_clone.acquire().await.unwrap();
                        //保存dbno的信息表
                        if !db_option_clone.only_rebuild_pdms_element {
                            let mut sql = format!("REPLACE INTO {PDMS_DBNO_INFOS_TABLE} ( NUMBDB,FILENAME,VERSION,PROJECT,DB_TYPE ) VALUES ");
                            sql.push_str(dbno_filename_sql.as_str());
                            let result = info_conn.execute(sql.as_str()).await;
                            match result {
                                Ok(_) => {}
                                Err(e) => {
                                    dbg!(&e);
                                    dbg!(sql.as_str());
                                }
                            }

                            //保存refno的信息表
                            let mut sql = format!("REPLACE INTO {PDMS_REFNO_INFOS_TABLE}(REF0, PROJECT) VALUES ");
                            for kv in &refno_info_map {
                                sql.push_str(&format!(r#"({},'{}') ,"#, kv.value().ref_0, /*v.db_no, */project.as_str()));
                            }
                            sql.remove(sql.len() - 1);
                            let result = info_conn.execute(sql.as_str()).await;
                            match result {
                                Ok(_) => {}
                                Err(e) => {
                                    dbg!(&e);
                                    dbg!(sql.as_str());
                                }
                            }
                        }

                        for (i, refno) in kv.value().iter().enumerate() {
                            let att = total_attr_map.get(refno).unwrap();
                            if implicit_query_data.is_none() {
                                implicit_query_data = Some(gen_implicit_attr_insert_sql(type_hash));
                            }
                            let column_hashs = &implicit_query_data.as_ref().unwrap().1;
                            // ref0_info_sql.push_str(&gen_refno_infos_insert_sql(*refno, &project_clones.clone()));
                            implicit_values_sql.push_str(&gen_implicit_attr_value_sql(att.value(), column_hashs));
                            explicit_values_sql.push_str(&gen_explicit_attr_value_sql(att.value()));
                            let name = get_name(&total_attr_map, &children_map, *refno).replace(r#"'"#, r#"\'"#)
                                .replace(r#"""#, r#"\""#);
                            let order = get_order(&total_attr_map, &children_map, *refno);
                            let children_count = children_map.get(refno).unwrap_or(&RefU64Vec::default()).len();
                            pdms_elements_sql.push_str(&gen_pdms_element_insert_sql(att.value(), &name, db_no.0, order, children_count));
                            //获取当前项目的连接
                            let mut project_conn = pool_clone.acquire().await.unwrap();

                            let mut insert_join_handles = vec![];
                            if (i != 0 && i % batch_chunks_cnt == 0) || i == (kv.value().len() - 1) {
                                let info_sql = take(&mut ref0_info_sql);
                                let implicit_values_sql = take(&mut implicit_values_sql);
                                let explicit_values_sql = take(&mut explicit_values_sql);
                                let pdms_elements_sql = take(&mut pdms_elements_sql);
                                let dbno_filename_sql = take(&mut dbno_filename_sql);
                                let implicit_query_data = implicit_query_data.clone();
                                let pool_clone = pool_clone.clone();
                                let info_pool_clone = info_pool_clone.clone();
                                let db_option_clone = db_option_clone.clone();
                                let insert_handle = tokio::spawn(async move {
                                    if !db_option_clone.only_rebuild_pdms_element {
                                        //执行隐式数据保存
                                        let mut sql = String::new();
                                        sql.push_str(implicit_query_data.as_ref().unwrap().0.as_str());
                                        sql.push_str(implicit_values_sql.as_str());
                                        sql.remove(sql.len() - 1);
                                        let result = project_conn.execute(sql.as_str()).await;
                                        match result {
                                            Ok(_) => {}
                                            Err(e) => {
                                                dbg!(&e);
                                                dbg!(sql.as_str());
                                            }
                                        }

                                        //执行显示数据保存
                                        let mut sql = format!("REPLACE INTO {PDMS_EXPLICIT_TABLE} (ID, REFNO, TYPE, OWNER, DATA) VALUES ");
                                        sql.push_str(explicit_values_sql.as_str());
                                        sql.remove(sql.len() - 1);
                                        let result = project_conn.execute(sql.as_str()).await;
                                        match result {
                                            Ok(_) => {}
                                            Err(e) => {
                                                dbg!(&e);
                                                dbg!(sql.as_str());
                                            }
                                        }
                                    }
                                    // {PDMS_ELEMENTS_TABLE} 保存
                                    let mut sql = format!("REPLACE INTO {PDMS_ELEMENTS_TABLE} (ID, REFNO, TYPE, OWNER, NAME, NUMBDB , ORDER_NUM,CHILDREN_COUNT, IS_DEL  ) VALUES ");
                                    sql.push_str(pdms_elements_sql.as_str());
                                    sql.remove(sql.len() - 1);
                                    let result = project_conn.execute(sql.as_str()).await;
                                    match result {
                                        Ok(_) => {}
                                        Err(e) => {
                                            dbg!(&e);
                                            dbg!(sql.as_str());
                                        }
                                    }
                                });

                                insert_join_handles.push(insert_handle);
                                if insert_join_handles.len() == batch_handles_cnt || i == (kv.value().len() - 1) {
                                    let insert_join_handles = take(&mut insert_join_handles);
                                    futures::future::join_all(insert_join_handles).await;
                                }
                            }
                        }
                    }
                    let mut project_conn = pool_clone.acquire().await.unwrap();
                    // 将带有 room_code 属性的保存下来
                    if !db_option_clone.only_rebuild_pdms_element {
                        for (room_name, refnos) in room_code_map.clone() {
                            // 将room_code单独存放到room_code表中
                            let mut room_code_sql = format!("REPLACE INTO {ROOM_CODE} (REFNO,ROOM_NAME) VALUES ");
                            for refno in refnos.clone() {
                                room_code_sql.push_str(&format!("( {},'{}' ) ,", refno.0, room_name.clone()));
                            }
                            room_code_sql.remove(room_code_sql.len() - 1);
                            let result = project_conn.execute(room_code_sql.as_str()).await;
                            match result {
                                Ok(_) => {}
                                Err(e) => {
                                    dbg!(&e);
                                    dbg!(room_code_sql.as_str());
                                }
                            }
                        }
                    }
                }
                // });
                // handles.push(handle);
                // });
            }
            // break;
        }
    }

// futures::future::join_all(handles).await;

    Ok(())
}
