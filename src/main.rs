use std::collections::{BTreeMap, HashSet};
use std::fmt::format;
use std::fs;
use std::mem::take;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use itertools::Itertools;
use aios_core::pdms_types::{AttrMap, AttrVal, NounHash, PdmsDatabaseInfo, RefI32Tuple, RefU64};
use aios_core::pdms_types::AttrVal::StringType;
use aios_core::tool::db_tool::db1_hash;
use dashmap::DashMap;
use parse_pdms_db::parse::{PdmsDbData, WholeAttMap};
use aios_database::{BATCH_CHUNKS_CNT, tables};
use parse_pdms_db::{db1_dehash, parse_file};
use parse_pdms_db::tool::hash_tool::{f32_round_2, f64_round_2, f64_round_3};
use sqlx::{MySql, MySqlPool, Pool};
use sqlx::pool::PoolConnection;
use aios_database::database::{get_connect_url, get_tidb_pool, init_database, init_info_database};
use aios_database::helper::{qualified_column_name, qualified_table_name};
use aios_database::options::DbOption;
use aios_database::consts::*;

use sqlx::Executor;
use aios_database::api::attr::insert_attr_info;
use aios_database::api::element::*;
use aios_database::api::project_mdb::insert_project_mdb;
use aios_database::tables::gen_create_attr_info_tables_sql;


#[macro_use]
extern crate clap;


pub const TYPE_HASH: u32 = db1_hash("TYPE");


pub async fn test_batch_insert(url: &str) {
    let connection = MySqlPool::connect(&url)
        .await
        .unwrap();
    let mut pool = connection.try_acquire().unwrap();
    let sql = format!(r#"INSERT {PDMS_ELEMENTS_TABLE} (id, refno, type, name) VALUES (1, 100, 'test', 'unset'), (2, 100, 'test', 'unset')"#);
    let result = sqlx::query(&sql).execute(&mut pool).await;
    match result {
        Ok(_) => {}
        Err(_) => {
            dbg!(sql.as_str());
        }
    }
}


#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    dbg!(&db_option);
    let mut time = Instant::now();

    let url = get_connect_url(&db_option.ip, &db_option.user, &db_option.password, "", &db_option.port);
    init_info_database(&get_connect_url(&db_option.ip, &db_option.user, &db_option.password, "", &db_option.port)).await;
    let pdms_info_pool = get_tidb_pool(&format!("{}/{}", url, PDMS_INFO_DB)).await;
    let mut pdms_info_conn = pdms_info_pool.clone().acquire().await?;
    let mut create_tables_elapse = 0;
    for project in &db_option.included_projects {
        init_database(project, &url).await;
        let project_conn_string = format!("{url}/{project}");
        let project_pool = get_tidb_pool(&project_conn_string).await;
        let mut conn = project_pool.acquire().await.unwrap();
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
                // tables_sql.push_str(&tables::gen_create_dbno_filename_tables_sql());
            }
        }
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
        create_tables_elapse += table_time.elapsed().as_millis();
        let result = pdms_info_conn.execute(tables::gen_create_dbno_filename_tables_sql().as_str()).await;
        match result {
            Ok(_) => {}
            Err(e) => {
                dbg!(&e);
                dbg!(tables_sql.as_str());
            }
        }
        if db_option.types_multi_thread {
            sync_total_async_threading(&db_option, project, project_pool.clone(), pdms_info_pool.clone()).await.expect("同步数据失败");
        } else {
            sync_total_async(&db_option, project, project_pool.clone(), pdms_info_pool.clone()).await.expect("同步数据失败");
        }
        insert_project_mdb(project_pool.clone(), pdms_info_pool.clone()).await?;
    }
    println!("创建表花费时间: {} ms", create_tables_elapse);
    println!("初始化数据库时间: {} ms", time.elapsed().as_millis() - create_tables_elapse);
    Ok(())
}

pub fn gen_explicit_att_insert_sql(refno: RefU64, type_name: &str, owner: RefU64, e_att: &AttrMap) -> String {
    let mut sql = String::new();
    let mut table_columns_sql = String::new();
    let table_name = type_name.replace("join", "joint");
    table_columns_sql.push_str("INSERT IGNORE INTO {PDMS_EXPLICIT_TABLE} (id, refno, type, owner, data)");

    let mut table_vals_sql = String::new();
    let data = hex::encode(bincode::serialize(e_att).unwrap());
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
    let data = hex::encode(bincode::serialize(&att.explicit_attmap).unwrap());
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
        let v = i_att.get(noun_hash).unwrap();

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


    table_vals_sql.remove(table_vals_sql.len() - 1);
    table_vals_sql.push_str("),");

    table_vals_sql
}

///多线程保存
pub async fn sync_total_async(db_option: &DbOption, project: &str, pool: Pool<MySql>, info_pool: Pool<MySql>) -> anyhow::Result<()> {
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
    let url = get_connect_url(&db_option.ip, &db_option.user, &db_option.password, "", &db_option.port);
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
                                     string_lookup,
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
                                    let mut sql = format!("INSERT IGNORE INTO {PDMS_ELEMENTS_TABLE} (id, refno, type, owner, name, dbno , order_num ) VALUES ");
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
pub async fn sync_total_async_threading(db_option: &DbOption, project: &str, pool: Pool<MySql>, info_pool: Pool<MySql>) -> anyhow::Result<()> {
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
    let url = get_connect_url(&db_option.ip, &db_option.user, &db_option.password, "", &db_option.port);
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
                                     string_lookup,
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
                                        let mut sql = format!("INSERT IGNORE INTO {PDMS_ELEMENTS_TABLE} (id, refno, type, owner, name, dbno , order_num ) VALUES ");
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