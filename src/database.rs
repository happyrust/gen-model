use std::collections::{BTreeMap, HashSet};
use std::fmt::format;
use std::time::Instant;
use aios_core::pdms_types::{AttrMap, AttrVal, NounHash, PdmsDatabaseInfo, RefU64};
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
use dashmap::DashMap;
use crate::api::project_mdb::insert_project_mdb;
use crate::consts::*;
use crate::{options, tables};
use crate::api::element::*;
use crate::helper::{qualified_column_name, qualified_table_name};
use crate::options::DbOption;
use sqlx::Executor;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::ssc::{gen_insert_ssc_node_sql, insert_set_ssc_node_sql};

pub trait MySqlMethods {
    fn add_to_args(&self, args: &mut sqlx::mysql::MySqlArguments);

    fn get_query(count: usize) -> anyhow::Result<String>;

    fn name() -> String;
}






//重新创建database
pub async fn init_database(project: &str, url: &str) -> anyhow::Result<()> {
    let connection = MySqlPool::connect(url)
        .await
        .unwrap();
    let mut pool = connection.try_acquire().unwrap();

    sqlx::query(&format!("drop database if exists {project}")).execute(&mut pool).await?;
    sqlx::query(&format!("create database {project} default charset utf8")).execute(&mut pool).await?;
    Ok(())
}

/// 创建 info 库和表
pub async fn init_info_database(url: &str) -> anyhow::Result<()> {
    let pool = MySqlPool::connect(&url).await?;
    pool.execute(format!("drop database if exists {PDMS_INFO_DB}; CREATE DATABASE IF NOT EXISTS {PDMS_INFO_DB};").as_str()).await?;

    let mut pool = AiosDBManager::get_db_pool(&url, PDMS_INFO_DB).await?;
    let mut sql = String::new();
    sql.push_str(&format!(r#"CREATE TABLE IF NOT EXISTS {} ("#, {PDMS_REFNO_INFOS_TABLE}));
    sql.push_str(&format!(r#"{} BIGINT NOT NULL PRIMARY KEY ,"#, "ref0"));
    sql.push_str(&format!(r#"{} VARCHAR(20)"#, "project"));

    sql.push_str(");");
    let result = pool.execute(sql.as_str()).await;
    match result {
        Ok(_) => {}
        Err(_) => {
            dbg!(sql.as_str());
        }
    }

    Ok(())
}


pub async fn sync_pdms(db_option: &DbOption) -> anyhow::Result<()>{
    println!("开始同部pdms/E3D: {} 的数据", &db_option.project_name);
    let mut time = Instant::now();
    let default_conn_str = AiosDBManager::get_default_conn_str(db_option);
    init_info_database(&default_conn_str).await?;
    let pdms_info_pool = AiosDBManager::get_db_pool(&default_conn_str, PDMS_INFO_DB).await?;
    let mut pdms_info_conn = pdms_info_pool.clone().acquire().await?;
    let mut create_tables_elapse = 0;
    for project in &db_option.included_projects {
        if db_option.recreate_db {
            init_database(project, &default_conn_str).await?;
            let mut table_time = Instant::now();
            let mut tables_sql = String::new();
            if let Ok(db_info) = serde_json::from_str::<PdmsDatabaseInfo>(&include_str!("../all_attr_info.json")) {
                for (k, v) in db_info.noun_attr_info_map {
                    let mut attr_map = BTreeMap::new();
                    let type_name = db1_dehash(k as u32).to_lowercase();
                    if type_name.is_empty() {
                        continue;
                    }
                    let mut tmp_sets = HashSet::new();
                    for (kk, vv) in v {
                        let att_name = vv.name.to_lowercase();
                        if att_name.starts_with(":") || vv.offset == 0 {
                            continue;
                        }
                        if !tmp_sets.contains(&att_name) {
                            tmp_sets.insert(att_name.clone());
                        } else {
                            continue;
                        }
                        if kk == TYPE_HASH as i32 {
                            attr_map.insert(vv.offset, (att_name, StringType(db1_dehash(k as u32).to_lowercase().into())));
                        } else {
                            attr_map.insert(vv.offset, (att_name, vv.default_val));
                        }
                    }
                    tables_sql.push_str(&tables::gen_create_implicit_tables_sql(type_name.as_str(), &attr_map));
                    tables_sql.push_str(&tables::gen_create_explicit_tables_sql());
                    tables_sql.push_str(&tables::gen_create_uda_tables_sql());
                }
            }

            let project_pool = AiosDBManager::get_db_pool(&default_conn_str, project).await?;
            let mut conn = project_pool.acquire().await?;

            tables_sql.push_str(&tables::gen_create_element_tables_sql());
            tables_sql.push_str(&tables::gen_create_project_mdb_sql());
            let result = conn.execute(tables_sql.as_str()).await;
            match result {
                Ok(_) => {}
                Err(e) => {
                    dbg!(&e);
                    dbg!(tables_sql.as_str());
                }
            }
            // 创建 ssc树的固定节点
            let result = conn.execute(tables::gen_create_ssc_element_tables_sql().as_str()).await;
            match result {
                Ok(_) => {}
                Err(e) => {
                    dbg!(&e);
                    dbg!(tables_sql.as_str());
                }
            }
            insert_set_ssc_node_sql(project_pool).await?;

            create_tables_elapse += table_time.elapsed().as_millis();
            let result = pdms_info_conn.execute(tables::gen_create_dbno_infos_tables_sql().as_str()).await;
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
            sync_total_async_threading(&db_option, project, project_pool.clone(), pdms_info_pool.clone()).await.expect("同步数据失败");
        } else {
            sync_total_async(&db_option, project, project_pool.clone(), pdms_info_pool.clone()).await.expect("同步数据失败");
        }
        insert_project_mdb(&project_pool, &pdms_info_pool).await?;
    }
    println!("创建表花费时间: {} ms", create_tables_elapse);
    println!("初始化数据库时间: {} ms", time.elapsed().as_millis() - create_tables_elapse);

    Ok(())
}

pub const TYPE_HASH: u32 = db1_hash("TYPE");

pub fn gen_explicit_att_insert_sql(refno: RefU64, type_name: &str, owner: RefU64, e_att: &AttrMap) -> String {
    let mut sql = String::new();
    let mut table_columns_sql = String::new();
    let table_name = type_name.replace("join", "joint");
    table_columns_sql.push_str("INSERT IGNORE INTO {PDMS_EXPLICIT_TABLE} (id, refno, type, owner, data)");

    let mut table_vals_sql = String::new();
    let data = hex::encode(e_att.into_compress_bytes());
    table_vals_sql.push_str(&format!(r#"({}, '{}', '{}', {}, 0x{})"#, refno.0, refno.to_refno_str(), table_name, owner.0, data));


    sql.push_str(&table_columns_sql);
    sql.push_str(" values ");
    sql.push_str(&table_vals_sql);

    sql
}

pub fn gen_implicit_attr_query_sql(att: &WholeAttMap) -> (String, Vec<NounHash>) {
    let i_att = &att.implicit_attmap;
    let type_name = i_att.get_type();
    let table_name = qualified_table_name(type_name);
    let mut table_columns_sql = String::new();
    table_columns_sql.push_str(&format!("INSERT IGNORE INTO {} (id, refno, type, owner,", table_name));

    let mut column_hashs = vec![];
    for (k, v) in &i_att.map {
        let mut att_name_full = db1_dehash(k.0).to_lowercase();
        if att_name_full.as_str() == "numbdb" {
            att_name_full = "dbno".to_string();
        }
        let att_name = qualified_column_name(att_name_full.as_str());
        if att_name.starts_with(":") || att_name.as_str() == "refno" || att_name.as_str() == "type" || att_name.as_str() == "owner" {
            continue;
        }
        match v {
            AttrVal::InvalidType => {}
            _ => {
                table_columns_sql.push_str(&format!("{},", att_name.as_str()));
                column_hashs.push(k.clone());
            }
        }
    }
    table_columns_sql.remove(table_columns_sql.len() - 1);
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

pub fn gen_implicit_attr_value_sql(att: &WholeAttMap, column_hashs: &Vec<NounHash>) -> String {
    let mut table_vals_sql = String::new();
    let i_att = &att.implicit_attmap;
    let refno = i_att.get_refno().unwrap();
    let type_name = i_att.get_type();
    let owner = i_att.get_owner().unwrap();
    table_vals_sql.push_str(&format!(r#"({}, '{}', '{}', {},"#, refno.0, refno.to_refno_str(), type_name, owner.0));
    for noun_hash in column_hashs {
        if let Some(v) = i_att.get(noun_hash) {
            match v {
                AttrVal::InvalidType => {}
                AttrVal::IntegerType(d) => {
                    table_vals_sql.push_str(&format!("{},", d.to_string()));
                }
                AttrVal::StringType(d) => {
                    table_vals_sql.push_str(&format!(r#"'{}',"#, d));
                }
                AttrVal::DoubleType(d) => {
                    table_vals_sql.push_str(&format!("{},", f64_round_3(*d)));
                }
                AttrVal::DoubleArrayType(d) => {
                    table_vals_sql.push_str(&format!(r#"0x{},"#, hex::encode(bincode::serialize(d).unwrap().as_slice())));
                }
                AttrVal::StringArrayType(d) => {
                    table_vals_sql.push_str(&format!(r#"'{}',"#, serde_json::to_string(d).unwrap()));
                }
                AttrVal::BoolArrayType(d) => {
                    table_vals_sql.push_str(&format!(r#"'{}',"#, serde_json::to_string(d).unwrap()));
                }
                AttrVal::IntArrayType(d) => {
                    table_vals_sql.push_str(&format!(r#"'{}',"#, serde_json::to_string(d).unwrap()));
                }
                AttrVal::BoolType(d) => {
                    let b = if *d { 1 } else { 0 };
                    table_vals_sql.push_str(&format!("{},", b));
                }
                AttrVal::Vec3Type(d) => {
                    table_vals_sql.push_str(&format!(r#"'{}',"#, serde_json::to_string(d).unwrap()));
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
                    table_vals_sql.push_str(&format!(r#"'{}',"#, serde_json::to_string(d).unwrap()));
                }
                AttrVal::StringHashType(_) => {}
            }
        }
    }


    table_vals_sql.remove(table_vals_sql.len() - 1);
    table_vals_sql.push_str("),");

    table_vals_sql
}

///多线程保存
pub async fn sync_total_async(db_option: &options::DbOption, project: &str, pool: Pool<MySql>, info_pool: Pool<MySql>) -> anyhow::Result<()> {
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
                            let mut implicit_query_data = None;
                            let mut ref0_info_sql = String::new();
                            let mut implicit_values_sql = String::new();
                            let mut explicit_values_sql = String::new();
                            let mut pdms_elements_sql = String::new();
                            let mut dbno_filename_sql = gen_dbno_filename_insert_sql(db_no.0, &filename_clone.clone(),
                                                                                     version.0, &project_clones, db_type.clone());
                            let mut info_conn = info_pool_clone.acquire().await.unwrap();
                            //保存dbno的信息表
                            let mut sql = format!("INSERT IGNORE INTO {PDMS_DBNO_INFOS_TABLE} ( dbno,filename,version,project,db_type ) VALUES ");
                            sql.push_str(dbno_filename_sql.as_str());
                            sql.remove(sql.len() - 1);
                            let result = info_conn.execute(sql.as_str()).await;
                            match result {
                                Ok(_) => {}
                                Err(e) => {
                                    dbg!(&e);
                                    dbg!(sql.as_str());
                                }
                            }

                            //保存refno的信息表
                            let mut sql = format!("INSERT IGNORE INTO {PDMS_REFNO_INFOS_TABLE} (ref0, project) VALUES ");
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


                            for (i, refno) in kv.value().iter().enumerate() {
                                let att = total_attr_map.get(refno).unwrap();
                                if implicit_query_data.is_none() {
                                    implicit_query_data = Some(gen_implicit_attr_query_sql(att.value()));
                                }
                                let column_hashs = &implicit_query_data.as_ref().unwrap().1;
                                // ref0_info_sql.push_str(&gen_refno_infos_insert_sql(*refno, &project_clones.clone()));
                                implicit_values_sql.push_str(&gen_implicit_attr_value_sql(att.value(), column_hashs));
                                explicit_values_sql.push_str(&gen_explicit_attr_value_sql(att.value()));
                                let name = get_name(&total_attr_map, &children_map, *refno).replace(r#"'"#, r#"\'"#)
                                    .replace(r#"""#, r#"\""#);
                                let order = get_order(&total_attr_map, &children_map, *refno);
                                pdms_elements_sql.push_str(&gen_pdms_element_insert_sql(att.value(), &name, db_no.0, order));
                                //获取当前项目的连接
                                let mut project_conn = pool_clone.acquire().await.unwrap();

                                // let mut insert_join_handles = vec![];
                                if (i != 0 && i % batch_chunks_cnt == 0) || i == (kv.value().len() - 1) {
                                    // dbg!(i % batch_chunks_cnt );
                                    let info_sql = take(&mut ref0_info_sql);
                                    let implicit_values_sql = take(&mut implicit_values_sql);
                                    let explicit_values_sql = take(&mut explicit_values_sql);
                                    let pdms_elements_sql = take(&mut pdms_elements_sql);
                                    let implicit_query_data = implicit_query_data.clone();
                                    //执行隐式数据保存
                                    let mut sql = String::new();
                                    sql.push_str(implicit_query_data.as_ref().unwrap().0.as_str());
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
                                    let mut sql = format!("INSERT IGNORE INTO {PDMS_EXPLICIT_TABLE} (id, refno, type, owner, data) VALUES ");
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
                                    // {PDMS_ELEMENTS_TABLE} 保存
                                    let mut sql = format!("INSERT IGNORE INTO {PDMS_ELEMENTS_TABLE} (id, refno, type, owner, name, dbno , order_num, is_del ) VALUES ");
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
                        // 将带room_code数据的refno存放在pdms_ssc_element中
                        let mut project_conn = pool_clone.acquire().await.unwrap();
                        for (room_name, refnos) in room_code_map {
                            let insert_sql = "insert ignore into pdms_ssc_elements (id, refno, type, owner, name, order_num) VALUES ";
                            let mut values_sql = String::new();
                            if let Ok(Some(owner_refno)) = query_id_from_name_ssc(&room_name, pool_clone.clone()).await {
                                let mut order = 0;
                                for refno in refnos {
                                    if let Some(total_attr) = &total_attr_map.get(&refno) {
                                        let type_name = total_attr.implicit_attmap.get_type();
                                        let name = get_name(&total_attr_map, &children_map, refno).replace(r#"'"#, r#"\'"#)
                                            .replace(r#"""#, r#"\""#).replace(r#"\"#, r#"\\"#);
                                        values_sql.push_str(&gen_insert_ssc_node_sql(refno, type_name, owner_refno, &name, order).1);
                                    }
                                    order += 1;
                                }
                                values_sql.remove(values_sql.len() - 1);
                                values_sql.push_str(";");
                                let sql = format!("{}{}", insert_sql, values_sql);
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
                            let mut implicit_query_data = None;
                            let mut ref0_info_sql = String::new();
                            let mut implicit_values_sql = String::new();
                            let mut explicit_values_sql = String::new();
                            let mut pdms_elements_sql = String::new();
                            let mut dbno_filename_sql = gen_dbno_filename_insert_sql(db_no.0, &filename_clone.clone(),
                                                                                     version.0, &project_clones, db_type.clone());
                            let mut info_conn = info_pool_clone.acquire().await.unwrap();
                            //保存dbno的信息表
                            let mut sql = format!("INSERT IGNORE INTO {PDMS_DBNO_INFOS_TABLE} ( dbno,filename,version,project,db_type ) VALUES ");
                            sql.push_str(dbno_filename_sql.as_str());
                            sql.remove(sql.len() - 1);
                            let result = info_conn.execute(sql.as_str()).await;
                            match result {
                                Ok(_) => {}
                                Err(e) => {
                                    dbg!(&e);
                                    dbg!(sql.as_str());
                                }
                            }

                            //保存refno的信息表
                            let mut sql = format!("INSERT IGNORE INTO {PDMS_REFNO_INFOS_TABLE} (ref0, project) VALUES ");
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

                            for (i, refno) in kv.value().iter().enumerate() {
                                let att = total_attr_map.get(refno).unwrap();
                                if implicit_query_data.is_none() {
                                    implicit_query_data = Some(gen_implicit_attr_query_sql(att.value()));
                                }
                                let column_hashs = &implicit_query_data.as_ref().unwrap().1;
                                // ref0_info_sql.push_str(&gen_refno_infos_insert_sql(*refno, &project_clones.clone()));
                                implicit_values_sql.push_str(&gen_implicit_attr_value_sql(att.value(), column_hashs));
                                explicit_values_sql.push_str(&gen_explicit_attr_value_sql(att.value()));
                                let name = get_name(&total_attr_map, &children_map, *refno).replace(r#"'"#, r#"\'"#)
                                    .replace(r#"""#, r#"\""#);
                                let order = get_order(&total_attr_map, &children_map, *refno);
                                pdms_elements_sql.push_str(&gen_pdms_element_insert_sql(att.value(), &name, db_no.0, order));
                                //获取当前项目的连接
                                let mut project_conn = pool_clone.acquire().await.unwrap();

                                let mut insert_join_handles = vec![];
                                if (i != 0 && i % batch_chunks_cnt == 0) || i == (kv.value().len() - 1) {
                                    // dbg!(i % batch_chunks_cnt );
                                    let info_sql = take(&mut ref0_info_sql);
                                    let implicit_values_sql = take(&mut implicit_values_sql);
                                    let explicit_values_sql = take(&mut explicit_values_sql);
                                    let pdms_elements_sql = take(&mut pdms_elements_sql);
                                    let dbno_filename_sql = take(&mut dbno_filename_sql);
                                    let implicit_query_data = implicit_query_data.clone();
                                    let pool_clone = pool_clone.clone();
                                    let info_pool_clone = info_pool_clone.clone();
                                    let insert_handle = tokio::spawn(async move {

                                        //执行隐式数据保存
                                        let mut sql = String::new();
                                        sql.push_str(implicit_query_data.as_ref().unwrap().0.as_str());
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
                                        let mut sql = format!("INSERT IGNORE INTO {PDMS_EXPLICIT_TABLE} (id, refno, type, owner, data) VALUES ");
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

                                        // {PDMS_ELEMENTS_TABLE} 保存
                                        let mut sql = format!("INSERT IGNORE INTO {PDMS_ELEMENTS_TABLE} (id, refno, type, owner, name, dbno , order_num, is_del  ) VALUES ");
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
                        // 将带有room_code属性的参考号放入pdms_ssc_elements中
                        let mut project_conn = pool_clone.acquire().await.unwrap();
                        for (room_name, refnos) in room_code_map {
                            let insert_sql = "insert ignore into pdms_ssc_elements (id, refno, type, owner, name, order_num) VALUES ";
                            let mut values_sql = String::new();
                            if let Ok(Some(owner_refno)) = query_id_from_name_ssc(&room_name, pool_clone.clone()).await {
                                let mut order = 0;
                                for refno in refnos {
                                    if let Some(total_attr) = &total_attr_map.get(&refno) {
                                        let type_name = total_attr.implicit_attmap.get_type();
                                        let name = get_name(&total_attr_map, &children_map, refno).replace(r#"'"#, r#"\'"#)
                                            .replace(r#"""#, r#"\""#).replace(r#"\"#, r#"\\"#);
                                        values_sql.push_str(&gen_insert_ssc_node_sql(refno, type_name, owner_refno, &name, order).1);
                                    }
                                    order += 1;
                                }
                                values_sql.remove(values_sql.len() - 1);
                                values_sql.push_str(";");
                                let sql = format!("{}{}", insert_sql, values_sql);
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
                });
                handles.push(handle);
            }
        }
        // break;
    }

    futures::future::join_all(handles).await;

    Ok(())
}
