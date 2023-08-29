use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::default;
use std::fmt::format;
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::mem::take;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use aios_core::consts::*;
use aios_core::pdms_types::AttrVal::StringType;
use aios_core::pdms_types::{
    AttrMap, AttrVal, NounHash, PdmsDatabaseInfo, PlantMeshesData, RefU64, RefU64Vec,
};
use aios_core::tool::db_tool::{convert_to_hash, db1_dehash, db1_hash};
use aios_core::tool::float_tool::f64_round_3;
use anyhow::anyhow;
use dashmap::{DashMap, DashSet};
use itertools::Itertools;
use nom::character::complete::u64;
use parse_pdms_db::parse::*;
use parse_pdms_db::parse_file;
use smol_str::SmolStr;
use sqlx::mysql::MySqlArguments;
use sqlx::pool::PoolConnection;
use sqlx::{Connection, MySql, MySqlPool, Pool};
use sqlx::{Error, Executor};

use crate::api::element::*;
use crate::api::ssc_data::SscEleNode;
use crate::aql_api::PdmsPLINAttrAql;
use crate::consts::*;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::*;
use crate::graph_db::{ForeignEdges, ParaDocument};
use crate::ssc::{gen_insert_ssc_node_sql, insert_set_ssc_node_sql, insert_ssc_room_node};
use crate::tables;
use crate::tables::*;
use aios_core::cache::mgr::BytesTrait;
use aios_core::get_default_pdms_db_info;
use aios_core::helper::table::{qualified_column_name, qualified_table_name};
use aios_core::options::DbOption;
use aios_core::pdms_data::ATTR_INFO_MAP;
use parry3d::utils::hashmap::FxHasher32;
use sled::transaction::ConflictableTransactionError;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::str::FromStr;
use id_tree::Tree;
use indexmap::IndexMap;

pub trait MySqlMethods {
    fn add_to_args(&self, args: &mut sqlx::mysql::MySqlArguments);

    fn get_query(count: usize) -> anyhow::Result<String>;

    fn name() -> String;
}

/// 初始化project database
pub async fn create_project_database(project: &str, url: &str) -> anyhow::Result<()> {
    let connection = MySqlPool::connect(url).await.unwrap();
    let mut pool = connection.try_acquire().unwrap();
    sqlx::query(&format!(
        "CREATE DATABASE IF NOT EXISTS {project} DEFAULT CHARSET UTF8"
    ))
    .execute(&mut pool)
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
    println!("开始同步pdms/E3D: {} 的数据", &db_option.project_name);
    let mut time = Instant::now();
    let default_conn_str = AiosDBManager::get_default_conn_str(db_option);
    create_info_database(&default_conn_str, &db_option.project_name).await?;


    let pdms_info_pool = AiosDBManager::get_db_pool(
        &default_conn_str,
        &format!("{}_{}", PDMS_INFO_DB, &db_option.project_name),
    )
    .await?;
    let mut create_tables_elapse = 0;
    dbg!("执行多线程解析");
    for project in &db_option.included_projects {
        create_project_database(project, &default_conn_str).await?;
        let project_pool = AiosDBManager::get_db_pool(&default_conn_str, project).await?;
        let mut table_time = Instant::now();
        let mut tables_sql = String::new();

        let (att_map_tree, children_tree) = {
            let db_path = format!("{}.db", &project);
            let config = sled::Config::default()
                .path(db_path)
                .mode(sled::Mode::HighThroughput)
                .cache_capacity(10_000_000_000)
                .flush_every_ms(Some(1000));
            let db = config.open()?;
            let tree = db.open_tree("attr_map")?;
            let children_tree = db.open_tree("children")?;
            (tree, children_tree)
        };

        let db_info = get_default_pdms_db_info();
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
                    if kk == TYPE_HASH as i32 {
                        attr_map.insert(
                            vv.offset,
                            (att_name, StringType(db1_dehash(k as u32).into())),
                        );
                    } else {
                        attr_map.insert(vv.offset, (att_name, vv.default_val));
                    }
                }
            }
            tables_sql.push_str(&tables::gen_create_implicit_tables_sql(
                type_name.as_str(),
                &attr_map,
            ));
            tables_sql.push_str(&tables::gen_create_explicit_tables_sql());
            tables_sql.push_str(&tables::gen_create_uda_tables_sql());
        }
        let mut conn = project_pool.acquire().await?;
        tables_sql.push_str(&tables::gen_create_element_tables_sql());
        tables_sql.push_str(&gen_create_project_mdb_sql());
        tables_sql.push_str(&gen_create_data_state_tables_sql());
        tables_sql.push_str(&gen_create_pdms_version_table_sql());
        tables_sql.push_str(&gen_create_room_code_table_sql());
        tables_sql.push_str(&gen_create_file_version_table_sql());
        let result = execute_sql(&mut conn, tables_sql.as_str()).await;
        create_tables_elapse += table_time.elapsed().as_millis();

        let project_pool = AiosDBManager::get_db_pool(&default_conn_str, project).await?;
        let arango_pool = connect_arangodb(db_option).await?;
        match sync_total_async_threaded(
            arango_pool.clone(),
            &db_option,
            project,
            project_pool.clone(),
            pdms_info_pool.clone(),
            att_map_tree.clone(),
            children_tree.clone(),
        ).await {
            Ok(_) => {
                println!("同步数据成功。");
            }
            Err(e) => {
                println!("{}", e.to_string());
            }
        }
    }
    println!("创建表花费时间: {} ms", create_tables_elapse);
    println!(
        "初始化数据库时间: {} ms",
        time.elapsed().as_millis() - create_tables_elapse
    );

    Ok(())
}

pub async fn execute_sql(conn: &mut PoolConnection<MySql>, sql: &str) -> bool {
    match conn.execute(sql).await {
        Ok(_) => {
            return true;
        }
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
            return false;
        }
    }
}

pub fn gen_explicit_att_insert_sql(
    refno: RefU64,
    type_name: &str,
    owner: RefU64,
    e_att: &AttrMap,
) -> String {
    let mut sql = String::new();
    let mut table_columns_sql = String::new();
    let table_name = qualified_table_name(type_name);
    // table_columns_sql.push_str("REPLACE INTO {PDMS_EXPLICIT_TABLE} (ID, REFNO, TYPE, OWNER, DATA)");
    table_columns_sql
        .push_str("INSERT IGNORE INTO {PDMS_EXPLICIT_TABLE} (ID, REFNO, TYPE, OWNER, DATA)");

    let mut table_vals_sql = String::new();
    let data = hex::encode(e_att.into_compress_bytes());
    table_vals_sql.push_str(&format!(
        r#"({}, '{}', '{}', {}, 0x{})"#,
        refno.0,
        refno.to_refno_str(),
        table_name,
        owner.0,
        data
    ));

    sql.push_str(&table_columns_sql);
    sql.push_str(" VALUES ");
    sql.push_str(&table_vals_sql);

    sql
}

/// 生成隐藏属性的插入语句的前面列名部分
pub fn gen_implicit_attr_insert_sql(hash: u32) -> (String, Vec<NounHash>) {
    let type_name = db1_dehash(hash);
    let table_name = qualified_table_name(type_name.as_str());
    let mut table_columns_sql = String::new();
    // if b_replace {
    //     table_columns_sql.push_str(&format!("REPLACE INTO {} (ID, REFNO, TYPE, OWNER", table_name));
    // } else {
    table_columns_sql.push_str(&format!(
        "INSERT IGNORE INTO {} (ID, REFNO, TYPE, OWNER",
        table_name
    ));
    // }

    let implicit_names = ATTR_INFO_MAP.get_type_implicit_att_names(type_name.as_str());
    let column_hashs = implicit_names
        .iter()
        .filter_map(|x| (x != "unset").then(|| (db1_hash(x.as_str()))))
        .collect();
    let v_sql = implicit_names
        .iter()
        .map(|x| qualified_column_name(x.as_str()))
        .join(",");
    // dbg!(&v_sql);
    if v_sql.len() > 0 {
        table_columns_sql.push_str(" , ");
    }
    table_columns_sql.push_str(v_sql.as_str());
    table_columns_sql.push_str(") VALUES ");

    (table_columns_sql, column_hashs)
}

#[inline]
pub fn gen_uda_attr_value_sql(att: &WholeAttMap) -> String {
    let mut table_vals_sql = String::new();
    let i_att = &att.implicit_attmap;
    let refno = i_att.get_refno().unwrap();
    let type_name = i_att.get_type();
    let owner = i_att.get_owner().unwrap();
    let data = hex::encode(att.uda_attmap.into_compress_bytes());
    table_vals_sql.push_str(&format!(
        r#"({}, '{}', '{}', {}, 0x{}),"#,
        refno.0,
        refno.to_refno_str(),
        type_name,
        owner.0,
        data
    ));

    table_vals_sql
}

#[inline]
pub fn gen_explicit_attr_value_sql(att: &WholeAttMap) -> String {
    let mut table_vals_sql = String::new();
    let i_att = &att.implicit_attmap;
    let refno = i_att.get_refno().unwrap();
    let type_name = i_att.get_type();
    let owner = i_att.get_owner().unwrap();
    let data = hex::encode(att.explicit_attmap.into_compress_bytes());
    table_vals_sql.push_str(&format!(
        r#"({}, '{}', '{}', {}, 0x{}),"#,
        refno.0,
        refno.to_refno_str(),
        type_name,
        owner.0,
        data
    ));

    table_vals_sql
}

/// 生成隐藏属性的插入语句的后面数据部分
pub fn gen_implicit_attr_value_sql(att: &WholeAttMap, column_hashes: &Vec<NounHash>) -> String {
    let mut table_vals_sql = String::new();
    let i_att = &att.implicit_attmap;
    let refno = i_att.get_refno().unwrap();
    let type_name = i_att.get_type();
    let owner = i_att.get_owner().unwrap();
    table_vals_sql.push_str(&format!(
        r#"({}, '{}', '{}', {},"#,
        refno.0,
        refno.to_refno_str(),
        type_name,
        owner.0
    ));
    if let Some(info_map) = ATTR_INFO_MAP.get(&(db1_hash(type_name) as i32)) {
        for noun_hash in column_hashes {
            //如果没有这个属性，需要用unset顶上
            //if noun_hash != &(UNSET_NOUN)
            if let Some(v) = i_att.get(noun_hash) {
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
                        table_vals_sql.push_str(&format!(
                            r#"0x{},"#,
                            hex::encode(bincode::serialize(d).unwrap_or_default().as_slice())
                        ));
                    }
                    AttrVal::StringArrayType(d) => {
                        table_vals_sql.push_str(&format!(
                            r#"'{}',"#,
                            serde_json::to_string(d).unwrap_or_default()
                        ));
                    }
                    AttrVal::BoolArrayType(d) => {
                        table_vals_sql.push_str(&format!(
                            r#"'{}',"#,
                            serde_json::to_string(d).unwrap_or_default()
                        ));
                    }
                    AttrVal::IntArrayType(d) => {
                        table_vals_sql.push_str(&format!(
                            r#"'{}',"#,
                            serde_json::to_string(d).unwrap_or_default()
                        ));
                    }
                    AttrVal::BoolType(d) => {
                        let b = if *d { 1 } else { 0 };
                        table_vals_sql.push_str(&format!("{},", b));
                    }
                    AttrVal::Vec3Type(d) => {
                        table_vals_sql.push_str(&format!(
                            r#"'{}',"#,
                            serde_json::to_string(d).unwrap_or_default()
                        ));
                    }
                    AttrVal::ElementType(d) => {
                        table_vals_sql.push_str(&format!(r#"'{}',"#, d.replace(r#"'"#, "")));
                    }
                    AttrVal::WordType(d) => {
                        table_vals_sql.push_str(&format!(r#"'{}',"#, d.replace(r#"'"#, "")));
                    }
                    AttrVal::RefU64Type(d) => {
                        table_vals_sql.push_str(&format!("{},", d.0));
                    }
                    AttrVal::RefU64Array(d) => {
                        table_vals_sql.push_str(&format!(
                            r#"'{}',"#,
                            serde_json::to_string(d).unwrap_or_default()
                        ));
                    }
                    AttrVal::StringHashType(_) => {}
                }
            } else {
                if let Some(info) = info_map.get(&(*noun_hash as i32)) {
                    // todo 和上面的 math 合并为一个

                    match &info.default_val {
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
                            table_vals_sql.push_str(&format!(
                                r#"0x{},"#,
                                hex::encode(bincode::serialize(d).unwrap_or_default().as_slice())
                            ));
                        }
                        AttrVal::StringArrayType(d) => {
                            table_vals_sql.push_str(&format!(
                                r#"'{}',"#,
                                serde_json::to_string(d).unwrap_or_default()
                            ));
                        }
                        AttrVal::BoolArrayType(d) => {
                            table_vals_sql.push_str(&format!(
                                r#"'{}',"#,
                                serde_json::to_string(d).unwrap_or_default()
                            ));
                        }
                        AttrVal::IntArrayType(d) => {
                            table_vals_sql.push_str(&format!(
                                r#"'{}',"#,
                                serde_json::to_string(d).unwrap_or_default()
                            ));
                        }
                        AttrVal::BoolType(d) => {
                            let b = if *d { 1 } else { 0 };
                            table_vals_sql.push_str(&format!("{},", b));
                        }
                        AttrVal::Vec3Type(d) => {
                            table_vals_sql.push_str(&format!(
                                r#"'{}',"#,
                                serde_json::to_string(d).unwrap_or_default()
                            ));
                        }
                        AttrVal::ElementType(d) => {
                            table_vals_sql.push_str(&format!(r#"'{}',"#, d.replace(r#"'"#, "")));
                        }
                        AttrVal::WordType(d) => {
                            table_vals_sql.push_str(&format!(r#"'{}',"#, d.replace(r#"'"#, "")));
                        }
                        AttrVal::RefU64Type(d) => {
                            table_vals_sql.push_str(&format!("{},", d.0));
                        }
                        AttrVal::RefU64Array(d) => {
                            table_vals_sql.push_str(&format!(
                                r#"'{}',"#,
                                serde_json::to_string(d).unwrap_or_default()
                            ));
                        }
                        AttrVal::StringHashType(_) => {}
                    }
                } else {
                    table_vals_sql.push_str(r#"'unset',"#);
                }
            }
        }
    }
    table_vals_sql.remove(table_vals_sql.len() - 1);
    table_vals_sql.push_str("),");

    table_vals_sql
}


///多线程同步数据，包括增量同步
pub async fn sync_total_async_threaded(
    arango_pool: ArPool,
    db_option: &DbOption,
    project: &str,
    pool: Pool<MySql>,
    info_pool: Pool<MySql>,
    attmap_tree: sled::Tree,
    children_tree: sled::Tree,
) -> anyhow::Result<()> {
    let mut data_dir = Path::new(&db_option.project_path);
    let need_parsed_files = &db_option.included_db_files;
    let project_dir = data_dir.join(&project);
    let max_sql_threads_number = db_option.sql_threads_number as usize;
    let batch_insert_sql_cnt = db_option.batch_insert_sql_cnt as usize;
    if max_sql_threads_number * batch_insert_sql_cnt == 0 {
        return Err(anyhow::anyhow!(
            "batch_insert_sql_cnt 或者  sql_threads_number 不能为0"
        ));
    }
    if !Path::new(&project_dir).exists() {
        return Err(anyhow::anyhow!("项目文件夹指定不正确"));
    }

    let mut children_files =  {
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

    let project = Arc::new(project.to_string());
    let db_option = Arc::new(db_option.clone());
    let mut error_sql = Arc::new(DashSet::new());
    //是否替换tidb的数据
    let mut is_replace = db_option.replace_dbs;
    let replace_types = db_option.replace_types.clone();
    let b_replace_types = replace_types.is_some();
    if b_replace_types {
        is_replace = true
    }
    let mut uda_map: HashMap<i32, AttrMap> = HashMap::new();
    let mut version_map = HashMap::new();
    let only_update_dbinfo = db_option.only_update_dbinfo;
    let only_sync_sys = db_option.only_sync_sys;
    const CHUNK_NUM: usize = 100_000;
    for path in children_files {
        let file_name = path.file_name().unwrap().to_str().unwrap().to_string();
        if file_name.ends_with("com") || file_name.ends_with("mis") {
            continue;
        }
        if only_sync_sys {
            if !file_name.ends_with("sys") {
                continue;
            }
        }
        if need_parsed_files.is_none() || need_parsed_files.as_ref().unwrap().contains(&file_name) {
            println!("path={:?}", &file_name);
            let project_clone = project.clone();
            let project_name = project.as_str().to_string();
            let mut children_map = parse_file_children_map(
                &path,
                &None,
                &file_name,
                project_name.clone().as_str(),
                "",
            )
            .unwrap_or_default();
            dbg!(children_map.len());
            let all_refnos = children_map.keys().cloned().collect::<Vec<_>>();
            dbg!(&all_refnos);
            let children_map_clone = Arc::new(children_map);

            if db_option.sync_graph_db.unwrap_or(true) {
                let database = arango_pool
                    .get()
                    .await?
                    .db(&db_option.arangodb_database)
                    .await?;
                save_pdms_level_edges_in_sync(&database, &children_map_clone).await?;
            }

            if db_option.sync_localdb.unwrap_or(true) {
                dbg!(children_map_clone.len());
                for (k, v) in children_map_clone.as_ref() {
                    let children_refnos = RefU64Vec(v.iter().map(|x| x.0).collect::<Vec<_>>());
                    let mut vec = children_refnos.to_bytes()?;
                    children_tree.insert((**k).to_be_bytes().as_slice(), &*vec)?;
                }
            }

            for (chunk_index, chunk_refnos) in all_refnos.chunks(CHUNK_NUM).enumerate() {
                let path_clone = path.clone();
                let file_name_clone = file_name.clone();
                let chunk_refnos_clone = chunk_refnos.to_vec();
                let project_name_clone = project_name.clone();
                if let Ok(Ok(PdmsDbData {
                    total_attr_map,
                    type_ele_map,
                    refno_info_map,
                    db_type,
                    db_no,
                    version,
                    room_code_map,
                    foreign_refnos_map,
                    ..
                })) = tokio::task::spawn_blocking(move || {
                    parse_file_with_chunk(
                        &path_clone,
                        &None,
                        &file_name_clone,
                        project_name_clone.as_str(),
                        "",
                        &chunk_refnos_clone,
                    )
                }).await {
                    println!("Processing {} chunk index: {chunk_index}", &file_name);
                    let mut dbinfo_value_sql = gen_dbinfo_value_insert_sql(
                        db_no,
                        &file_name,
                        version,
                        project_clone.clone().as_str(),
                        db_type.clone(),
                    );
                    let mut info_conn = info_pool.acquire().await.unwrap();

                    //保存dbno的信息表
                    let mut sql = format!("REPLACE INTO {PDMS_DBNO_INFOS_TABLE} ( id, NUMBDB, FILENAME,VERSION,PROJECT,DB_TYPE ) VALUES ");
                    sql.push_str(dbinfo_value_sql.as_str());
                    if is_replace {
                        sql = sql.replace("INSERT IGNORE", "REPLACE");
                    }
                    let result = info_conn.execute(sql.as_str()).await;
                    match result {
                        Ok(_) => {}
                        Err(e) => {
                            dbg!(&e);
                            dbg!(sql.as_str());
                        }
                    }
                    //保存refno的信息表
                    let mut sql = format!(
                        "INSERT IGNORE INTO {PDMS_REFNO_INFOS_TABLE} (ID, REF0, PROJECT) VALUES "
                    );
                    for kv in &refno_info_map {
                        let mut s: FxHasher32 = Default::default();
                        kv.value().ref_0.hash(&mut s);
                        project_clone.hash(&mut s);
                        let h = s.finish();
                        sql.push_str(&format!(
                            r#"({}, {},'{}') ,"#,
                            h,
                            kv.value().ref_0,
                            project_clone.as_str()
                        ));
                    }
                    sql.remove(sql.len() - 1);
                    if is_replace {
                        sql = sql.replace("INSERT IGNORE", "REPLACE");
                    }
                    let result = info_conn.execute(sql.as_str()).await;
                    match result {
                        Ok(_) => {}
                        Err(e) => {
                            dbg!(&e);
                            dbg!(sql.as_str());
                        }
                    }

                    version_map.entry(file_name.clone()).or_insert(version);
                    set_uda_attr(&type_ele_map, &total_attr_map, &mut uda_map)?;

                    total_attr_map.iter_mut().for_each(|mut x|{
                        let name = cal_default_name(*x.key(), x.value(), &children_map_clone);
                        x.value_mut().explicit_attmap.insert(NAME_HASH, AttrVal::StringType(name));
                    });

                    //类型暂时不多线程
                    let total_attr_map_arc = Arc::new(total_attr_map);
                    let children_map_arc = children_map_clone.clone();
                    let mut type_handles = vec![];
                    // 将部分数据保存到图数据库
                    if db_option.sync_graph_db.unwrap_or(true) {
                        if db_type == "CATA" || db_type == "DESI" {
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
                            ).await?;

                            save_foreign_refno_edges_in_sync(&database, foreign_refnos_map).await?;
                            // 单独保存plin
                            save_plin_attr_arangodb(&database, &type_ele_map, &total_attr_map_arc).await?;
                            // 将 para 和 des_para保存的图数据库中
                            save_paras_into_arangodb(&database, &total_attr_map_arc).await?;
                            // 将 dtse下的data部分数据保存到图数据库
                            save_dtse_value_to_arangodb(
                                &database,
                                &type_ele_map,
                                &total_attr_map_arc,
                            ).await?;
                        }
                        println!("图数据库保存完成");
                    }

                    if db_option.sync_localdb.unwrap_or(true) {
                        for kv in total_attr_map_arc.as_ref() {
                            let mut vec = kv
                                .value()
                                .merge()
                                .into_rkyv_compress_bytes();

                            attmap_tree.insert((**kv.key()).to_be_bytes().as_slice(), &*vec)?;
                        }

                        // let c = total_attr_map_arc.clone();
                        // let tree = db.open_tree("attr_map")?;
                        // tree.transaction::<_, _, ()>(|tx_db| {
                        //     for kv in c.as_ref() {
                        //         let mut vec = kv.value().merge().into_bytes();
                        //         tx_db.insert((**kv.key()).to_be_bytes().as_slice(), &*vec)?;
                        //     }
                        //     Ok(())
                        //     // Ok::<(), ConflictableTransactionError<_>>(())
                        // }).expect("Error inserting local db ");
                    }

                    //heed lmdb 的实现
                    // if db_option.sync_localdb.unwrap_or(true) {
                    //     use heed::bytemuck::{Pod, Zeroable};
                    //     use heed::byteorder::BE;
                    //     use heed::types::*;
                    //     use heed::{Database, EnvOpenOptions};
                    //     let path = Path::new(".").join(format!("{}.mdb", &project_clone));
                    //
                    //     fs::create_dir_all(&path)?;
                    //
                    //     let env = EnvOpenOptions::new()
                    //         .map_size(10 * 1024 * 1024 * 1024) // 10MB
                    //         .max_dbs(3000)
                    //         .open(path)?;
                    //
                    //     // you can specify that a database will support some typed key/data
                    //     //
                    //     // like here we specify that the key will be an array of two i32
                    //     // and the data will be an str
                    //     let mut wtxn = env.write_txn()?;
                    //     let db: Database<U64<BE>, heed::types::ByteSlice> = env.create_database(&mut wtxn, Some("att"))?;
                    //     for kv in total_attr_map_arc.as_ref() {
                    //         let mut vec = kv.value().merge().into_bytes();
                    //         db.put(&mut wtxn, &**kv.key(), &*vec)?;
                    //     }
                    //     wtxn.commit()?;
                    //     println!("保存到本地db完成");
                    // }

                    //redb 的实现
                    // if db_option.sync_localdb.unwrap_or(true) {
                    //     use redb::{Database, Error, ReadableTable, TableDefinition};
                    //     let db = Database::create(format!("{}.redb", &project_clone))?;
                    //     // let table_name = format!("{db_no}");
                    //     let table_name = format!("kv");
                    //     let table: TableDefinition<u64, &[u8]> = TableDefinition::new(&table_name);
                    //     let write_txn = db.begin_write()?;
                    //     {
                    //         let mut table = write_txn.open_table(table)?;
                    //         for kv in total_attr_map_arc.as_ref() {
                    //             let mut vec = kv.value().merge().into_bytes();
                    //             table.insert(**kv.key(), &*vec)?;
                    //         }
                    //     }
                    //     write_txn.commit()?;
                    //     println!("保存到本地db完成");
                    // }

                    //如果不需要同步tidb，continue
                    if !db_option.sync_tidb.unwrap_or(true) {
                        continue;
                    }

                    for (type_hash, type_refnos) in type_ele_map {
                        if b_replace_types {
                            let replace_types = replace_types.clone().unwrap();
                            let att_type = db1_dehash(type_hash);
                            if !replace_types.contains(&att_type) {
                                continue;
                            }
                        }
                        let total_attr_map_arc = total_attr_map_arc.clone();
                        let children_map_arc = children_map_arc.clone();
                        let pool_clone = pool.clone();
                        let arango_pool_clone = arango_pool.clone();
                        let error_sql_clone = error_sql.clone();
                        // println!("类型: {} 数量: {}", db1_dehash(type_hash), type_refnos.len());

                        let type_handle = tokio::spawn(async move {
                            let refnos_cnt = type_refnos.len();
                            // 线程初步估计数量
                            let mut threads_cnt = refnos_cnt / (batch_insert_sql_cnt * 5) + 1;
                            threads_cnt = threads_cnt.min(max_sql_threads_number);
                            let thread_chunks_cnt = refnos_cnt / threads_cnt + 1;
                            let mut handles = vec![];
                            let all_refnos = Arc::new(type_refnos.into_iter().collect::<Vec<_>>());

                            for i in 0..threads_cnt as usize {
                                let total_attr_map_arc_clone = total_attr_map_arc.clone();
                                let children_map_arc_clone = children_map_arc.clone();
                                let all_refnos = all_refnos.clone();
                                let pool_clone = pool_clone.clone();
                                let arango_pool_clone = arango_pool_clone.clone();
                                let error_sql_clone = error_sql_clone.clone();
                                let mut implicit_values_sql = String::new();
                                let mut explicit_values_sql = String::new();
                                let mut pdms_elements_sql = String::new();
                                let insert_handle = tokio::spawn(async move {
                                    let start_idx = i * thread_chunks_cnt;
                                    let mut end_idx = start_idx + thread_chunks_cnt;
                                    if end_idx > refnos_cnt {
                                        end_idx = refnos_cnt;
                                    }

                                    let implicit_columns_sql =
                                        gen_implicit_attr_insert_sql(type_hash);
                                    let column_hashs = &implicit_columns_sql.1;
                                    for j in (start_idx..end_idx)
                                        .into_iter()
                                        .step_by(batch_insert_sql_cnt)
                                    {
                                        let mut end = j + batch_insert_sql_cnt;
                                        if end > refnos_cnt {
                                            end = refnos_cnt;
                                        }
                                        //合并sql语句
                                        for k in j..end {
                                            let refno = all_refnos[k];
                                            let att = total_attr_map_arc_clone.get(&refno).unwrap();

                                            implicit_values_sql.push_str(
                                                &gen_implicit_attr_value_sql(
                                                    att.value(),
                                                    column_hashs,
                                                ),
                                            );
                                            explicit_values_sql.push_str(
                                                &gen_explicit_attr_value_sql(att.value()),
                                            );
                                            let name = att.get_name()
                                            .replace(r#"'"#, r#"\'"#)
                                            .replace(r#"""#, r#"\""#);
                                            let order = get_order(
                                                refno,
                                                att.value(),
                                                &children_map_arc_clone,
                                            );
                                            let children_count = children_map_arc_clone
                                                .get(&refno)
                                                .map(|x| x.len())
                                                .unwrap_or_default();
                                            pdms_elements_sql.push_str(
                                                &gen_pdms_element_insert_sql(
                                                    att.value(),
                                                    &name,
                                                    db_no,
                                                    order,
                                                    children_count,
                                                ),
                                            );
                                        }
                                        if !only_update_dbinfo {
                                            let mut project_conn =
                                                pool_clone.acquire().await.unwrap();
                                            let mut sql = String::new();
                                            sql.push_str(implicit_columns_sql.0.as_str());
                                            sql.push_str(implicit_values_sql.as_str());
                                            sql.remove(sql.len() - 1);
                                            if is_replace {
                                                sql = sql.replace("INSERT IGNORE", "REPLACE");
                                            }
                                            let result = project_conn.execute(sql.as_str()).await;
                                            match result {
                                                Ok(_) => {}
                                                Err(e) => {
                                                    dbg!(&e);
                                                    dbg!(sql.as_str());
                                                    error_sql_clone.insert(sql);
                                                }
                                            }

                                            //执行显示数据保存
                                            let mut sql = format!("INSERT IGNORE INTO {PDMS_EXPLICIT_TABLE} (ID, REFNO, TYPE, OWNER, DATA) VALUES ");
                                            sql.push_str(explicit_values_sql.as_str());
                                            sql.remove(sql.len() - 1);
                                            if is_replace {
                                                sql = sql.replace("INSERT IGNORE", "REPLACE");
                                            }
                                            let result = project_conn.execute(sql.as_str()).await;
                                            match result {
                                                Ok(_) => {}
                                                Err(e) => {
                                                    dbg!(&e);
                                                    dbg!(sql.as_str());
                                                    error_sql_clone.insert(sql);
                                                }
                                            }

                                            // {PDMS_ELEMENTS_TABLE} 保存
                                            let mut sql = format!("INSERT IGNORE INTO {PDMS_ELEMENTS_TABLE} (ID, REFNO, TYPE, OWNER, NAME, NUMBDB , ORDER_NUM,CHILDREN_COUNT, IS_DEL  ) VALUES ");
                                            sql.push_str(pdms_elements_sql.as_str());
                                            sql.remove(sql.len() - 1);
                                            if is_replace {
                                                sql = sql.replace("INSERT IGNORE", "REPLACE");
                                            }
                                            let result = project_conn.execute(sql.as_str()).await;
                                            match result {
                                                Ok(_) => {}
                                                Err(e) => {
                                                    dbg!(&e);
                                                    dbg!(sql.as_str());
                                                    error_sql_clone.insert(sql);
                                                }
                                            }
                                        }
                                        implicit_values_sql.clear();
                                        explicit_values_sql.clear();
                                        pdms_elements_sql.clear();
                                    }
                                });
                                handles.push(insert_handle);
                            }
                            futures::future::join_all(take(&mut handles)).await;
                        });

                        type_handles.push(type_handle);
                    }

                    futures::future::join_all(take(&mut type_handles)).await;

                    let mut project_conn = pool.acquire().await.unwrap();
                    // 将带有 room_code 属性的保存下来
                    if !db_option.only_update_dbinfo {
                        for (room_name, refnos) in room_code_map.clone() {
                            let mut room_code_sql =
                                format!("INSERT IGNORE INTO {ROOM_CODE} (REFNO,ROOM_NAME) VALUES ");
                            for refno in refnos.clone() {
                                room_code_sql.push_str(&format!(
                                    "( {},'{}' ) ,",
                                    refno.0,
                                    room_name.clone()
                                ));
                            }
                            room_code_sql.remove(room_code_sql.len() - 1);
                            if is_replace {
                                room_code_sql = room_code_sql.replace("INSERT IGNORE", "REPLACE");
                            }
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
            }
        }
    }
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
    // 保存每个file最新的page_num
    if version_map.len() > 0 {
        let table_sql = gen_create_file_version_table_sql();
        let mut version_sql =
            format!("INSERT IGNORE INTO {PDMS_FILE_VERSION_TABLE} (FILENAME,VERSION) VALUES");
        for (file_name, version) in version_map.into_iter() {
            version_sql.push_str(&format!("('{}',{}),", file_name, version))
        }
        let mut project_conn = pool.acquire().await.unwrap();
        version_sql.remove(version_sql.len() - 1);
        let result = project_conn.execute(table_sql.as_str()).await;
        match result {
            Ok(_) => {}
            Err(e) => {
                dbg!(&e);
                dbg!(table_sql.as_str());
            }
        }
        let result = project_conn.execute(version_sql.as_str()).await;
        match result {
            Ok(_) => {}
            Err(e) => {
                dbg!(&e);
                dbg!(version_sql.as_str());
            }
        }
    }
    if !error_sql.is_empty() {
        // 重新执行有问题的sql
        println!("正在重新插入有问题的sql语句, 共 {} 条", error_sql.len());
        let mut conn = pool.acquire().await?;
        for sql in error_sql.iter() {
            let _ = conn.execute(sql.key().as_str()).await;
            // if r.is_ok() {
            //     error_sql.remove(sql.key());
            // }
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
    if let Some(uda_refnos) = type_ele_map.get(&db1_hash("UDA")) {
        // 获取每个 uda 的 ELEL , DFLT , UDNA属性
        for uda_refno in uda_refnos.value() {
            let uda_att = total_attr_map.get(uda_refno);
            if uda_att.is_none() {
                continue;
            }
            let uda_att = uda_att.unwrap();
            let uda_implicit_att = &uda_att.implicit_attmap;
            let uda_explicit_att = &uda_att.explicit_attmap;

            let ukey = uda_implicit_att.get_i32("UKEY");
            if ukey.is_none() {
                continue;
            }
            let ukey = ukey.unwrap();
            // let mut udna = uda_implicit_att.get_str("UDNA");
            // if udna == Some("") {
            //     udna = uda_explicit_att.get_str("DYUDNA");
            // }
            let elel = uda_explicit_att.get_i32_vec("ELEL");
            let default = uda_explicit_att.get_val("DFLT");
            if elel.is_none() || default.is_none() {
                continue;
            }
            // let udna = udna.unwrap();
            let elel = elel.unwrap();
            let default = default.unwrap();
            for noun in elel {
                uda_map
                    .entry(noun)
                    .or_insert_with(AttrMap::default)
                    .entry((ukey as u32))
                    .or_insert(default.clone());
            }
        }
    }
    Ok(())
}

/// 将部分type的数据单独保存到图数据库中
async fn save_plin_attr_arangodb(
    database: &ArDatabase,
    type_ele_map: &DashMap<u32, HashSet<RefU64>>,
    total_attr_map: &DashMap<RefU64, WholeAttMap>,
) -> anyhow::Result<()> {
    let mut refno_attrs = vec![];
    if let Some(refnos) = &type_ele_map.get(&db1_hash("PLIN")) {
        for refno in refnos.value() {
            let whole_attr = total_attr_map.get(refno);
            if whole_attr.is_none() {
                continue;
            }
            // 暂时只要 p_key 和 plaxis
            refno_attrs.push(PdmsPLINAttrAql {
                _key: refno.to_url_refno(),
                attr: whole_attr.unwrap().merge(),
            })
        }
        if refno_attrs.len() > 0 {
            let json = serde_json::to_value(&take(&mut refno_attrs))?;
            save_arangodb_with_db_option(database, json, "plin_eles").await?;
        }
    }
    Ok(())
}

async fn save_paras_into_arangodb(
    database: &ArDatabase,
    total_attr_map: &DashMap<RefU64, WholeAttMap>,
) -> anyhow::Result<()> {
    let mut para_map = Vec::new();
    let mut des_para_map = Vec::new();
    for v in total_attr_map.iter() {
        // para 和 des_para 都存在显示属性里
        let explicit_map = &v.explicit_attmap;
        if let Some(para) = explicit_map.get_val("PARA") {
            let paras = para.dvec_value();
            if paras.is_none() {
                continue;
            }
            para_map.push(ParaDocument {
                _key: v.key().to_url_refno(),
                para: paras.unwrap(),
            })
        } else if let Some(des_para) = explicit_map.get_val("DESP") {
            let paras = des_para.dvec_value();
            if paras.is_none() {
                continue;
            }
            des_para_map.push(ParaDocument {
                _key: v.key().to_url_refno(),
                para: paras.unwrap(),
            })
        }
    }
    for para in para_map.chunks(ARANGODB_SAVE_AMOUNT) {
        let para_json = serde_json::to_value(para)?;
        save_arangodb_with_db_option(database, para_json, "para_eles").await?;
    }
    for des_para in des_para_map.chunks(ARANGODB_SAVE_AMOUNT) {
        let des_para_json = serde_json::to_value(des_para)?;
        save_arangodb_with_db_option(database, des_para_json, "despara_eles").await?;
    }
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
