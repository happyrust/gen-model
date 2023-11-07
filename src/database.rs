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
use crate::surreal_service::{SUL_DB, SUL_DB_ASYNC};
use crate::tables;
use crate::tables::*;
use crate::versioned_db::client::*;
use aios_core::cache::mgr::BytesTrait;
use aios_core::helper::table::{qualified_column_name, qualified_table_name};
use aios_core::options::DbOption;
use aios_core::pdms_data::ATTR_INFO_MAP;
use aios_core::AttrVal::StringType;
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
    let mut create_tables_elapse = 0;
    // 执行多线程解析
    dbg!("执行多线程解析");
    // 遍历所有包含的项目
    for project in &db_option.included_projects {
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

        if db_option.sync_versioned.unwrap_or(true) {
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

        if db_option.sync_tidb.unwrap_or(false) {
            create_project_database(project, &default_conn_str).await?;

            let project_pool = AiosDBManager::get_db_pool(&default_conn_str, project).await?;
            let mut table_time = Instant::now();
            let mut tables_sql = String::new();
            let db_info = get_default_pdms_db_info();
            for (k, v) in db_info.noun_attr_info_map.clone() {
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
            let mut conn = project_pool;
            tables_sql.push_str(&tables::gen_create_element_tables_sql());
            tables_sql.push_str(&gen_create_project_mdb_sql());
            tables_sql.push_str(&gen_create_data_state_tables_sql());
            tables_sql.push_str(&gen_create_pdms_version_table_sql());
            tables_sql.push_str(&gen_create_room_code_table_sql());
            tables_sql.push_str(&gen_create_file_version_table_sql());
            let result = execute_sql(&mut conn, tables_sql.as_str()).await;
            create_tables_elapse += table_time.elapsed().as_millis();
        }

        // let project_pool = AiosDBManager::get_db_pool(&default_conn_str, project).await?;
        // let pdms_info_pool = AiosDBManager::get_db_pool(
        //     &default_conn_str,
        //     &format!("{}_{}", PDMS_INFO_DB, &db_option.project_name),
        // ).await?;

        match sync_total_async_threaded(
            // arango_pool.clone(),
            &db_option,
            project,
            // project_pool.clone(),
            // pdms_info_pool.clone(),
            att_map_tree.clone(),
            children_tree.clone(),
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

    //都结束之后再考虑更新record link，有些外键的type，后面才知道
    //todo 最后加入index
    // DEFINE INDEX userNameIndex ON TABLE user COLUMNS name SEARCH ANALYZER ascii BM25 HIGHLIGHTS;
    // 添加 relate 和 record link
    SUL_DB
        .query(include_str!("../schemas/do_relate_pe.surql"))
        .await.unwrap();

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
    let refno = i_att.get_refno().unwrap(); // 获取引用号
    let type_name = i_att.get_type(); // 获取类型名称
    let owner = i_att.get_owner().unwrap(); // 获取所有者
    let data = hex::encode(att.uda_attmap.into_compress_bytes()); // 将uda_attmap转换为压缩字节并进行十六进制编码
    table_vals_sql.push_str(&format!(
        r#"({}, '{}', '{}', {}, 0x{}),"#, // 插入语句模板
        refno.0,                          // 引用号的第一个元素
        refno.to_refno_str(),             // 引用号的字符串表示
        type_name,                        // 类型名称
        owner.0,                          // 所有者的第一个元素
        data                              // 数据
    ));
    table_vals_sql
}

#[inline]
pub fn gen_explicit_attr_value_sql(att: &WholeAttMap) -> String {
    // 创建一个空字符串，用于存储生成的SQL语句
    let mut table_vals_sql = String::new();
    // 获取implicit_attmap字段的引用
    let i_att = &att.implicit_attmap;
    // 获取refno字段的值，并确保其存在
    let refno = i_att.get_refno().unwrap();
    // 获取type字段的值
    let type_name = i_att.get_type();
    // 获取owner字段的值，并确保其存在
    let owner = i_att.get_owner().unwrap();
    // 将explicit_attmap字段转换为压缩字节数组，并将其转换为十六进制字符串
    let data = hex::encode(att.explicit_attmap.into_compress_bytes());
    // 构建SQL语句，并将其添加到table_vals_sql字符串中
    table_vals_sql.push_str(&format!(
        r#"({}, '{}', '{}', {}, 0x{}),"#,
        refno.0,
        refno.to_refno_str(),
        type_name,
        owner.0,
        data
    ));
    // 返回生成的SQL语句
    table_vals_sql
}

/// 生成隐藏属性的插入语句的后面数据部分
pub fn gen_implicit_attr_value_sql(att: &WholeAttMap, column_hashes: &Vec<NounHash>) -> String {
    let mut table_vals_sql = String::new(); // 创建一个空字符串，用于存储生成的SQL语句
    let i_att = &att.implicit_attmap; // 获取implicit_attmap字段的引用
    let refno = i_att.get_refno().unwrap(); // 获取refno字段的值，并确保其存在
    let type_name = i_att.get_type(); // 获取type字段的值
    let owner = i_att.get_owner().unwrap(); // 获取owner字段的值，并确保其存在

    table_vals_sql.push_str(&format!(
        r#"({}, '{}', '{}', {},"#, // 构建SQL语句的前半部分
        refno.0,                   // 将refno的整数值添加到SQL语句中
        refno.to_refno_str(),      // 将refno的字符串值添加到SQL语句中
        type_name,                 // 将type_name的值添加到SQL语句中
        owner.0                    // 将owner的整数值添加到SQL语句中
    ));
    if let Some(info_map) = ATTR_INFO_MAP.get(&(db1_hash(type_name) as i32)) {
        // 检查是否存在属性信息映射
        for noun_hash in column_hashes {
            // 遍历column_hashes中的每个属性哈希值
            // 如果没有这个属性，需要用unset顶上
            if (type_name == "UDA" || type_name == "UDET") && noun_hash == &db1_hash("UDNA") {
                // 检查是否为 "UDA" 或 "UDET" 类型，并且属性哈希值为 "UDNA"
                let uda = if i_att.contains_attr_name("UDNA") {
                    // 检查是否存在属性名称为 "UDNA"
                    let uda = i_att.get_str("UDNA").unwrap();
                    if uda.is_empty() {
                        att.explicit_attmap
                            .get_str("DYUDNA")
                            .unwrap_or("")
                            .to_string()
                    } else {
                        uda.to_string()
                    }
                } else {
                    "".to_string()
                };
                table_vals_sql.push_str(&format!("'{}',", uda.to_string()));
            } else if i_att.contains_attr_hash(*noun_hash) {
                // 检查是否存在属性哈希值对应的属性
                let v = i_att.get(noun_hash).unwrap();
                // if let Some(v) = i_att.get(noun_hash) {
                // match v {
                // // 根据属性值的类型进行匹配
                // AttrVal::InvalidType => {}
                // AttrVal::IntegerType(d) => {
                //     // 将整数类型属性值添加到table_vals_sql字符串中
                //     table_vals_sql.push_str(&format!("{},", d.to_string()));
                // }
                // AttrVal::StringType(d) => {
                //     // 将字符串类型属性值添加到table_vals_sql字符串中
                //     table_vals_sql.push_str(&format!(r#"'{}',"#, d.replace(r#"'"#, "")));
                // }
                // AttrVal::DoubleType(d) => {
                //     // 将浮点数类型属性值添加到table_vals_sql字符串中（保留3位小数）
                //     table_vals_sql.push_str(&format!("{},", f64_round_3(*d)));
                // }
                // AttrVal::DoubleArrayType(d) => {
                //     // 将双精度浮点数数组类型属性值序列化为字节数组，并将其转换为十六进制字符串后添加到table_vals_sql字符串中
                //     table_vals_sql.push_str(&format!(
                //         r#"0x{},"#,
                //         hex::encode(bincode::serialize(d).unwrap_or_default().as_slice())
                //     ));
                // }
                // AttrVal::StringArrayType(d) => {
                //     // 将字符串数组类型属性值序列化为JSON字符串后添加到table_vals_sql字符串中
                //     table_vals_sql.push_str(&format!(
                //         r#"'{}',"#,
                //         serde_json::to_string(d).unwrap_or_default()
                //     ));
                // }
                // AttrVal::BoolArrayType(d) => {
                //     // 将布尔数组类型属性值序列化为JSON字符串后添加到table_vals_sql字符串中
                //     table_vals_sql.push_str(&format!(
                //         r#"'{}',"#,
                //         serde_json::to_string(d).unwrap_or_default()
                //     ));
                // }
                // AttrVal::IntArrayType(d) => {
                //     // 将整数数组类型属性值序列化为JSON字符串后添加到table_vals_sql字符串中
                //     table_vals_sql.push_str(&format!(
                //         r#"'{}',"#,
                //         serde_json::to_string(d).unwrap_or_default()
                //     ));
                // }
                // AttrVal::BoolType(d) => {
                //     // 将布尔类型属性值转换为整数（1或0）后添加到table_vals_sql字符串中
                //     let b = if *d { 1 } else { 0 };
                //     table_vals_sql.push_str(&format!("{},", b));
                // }
                // AttrVal::Vec3Type(d) => {
                //     // 将Vec3类型属性值序列化为JSON字符串后添加到table_vals_sql字符串中
                //     table_vals_sql.push_str(&format!(
                //         r#"'{}',"#,
                //         serde_json::to_string(d).unwrap_or_default()
                //     ));
                // }
                // AttrVal::ElementType(d) => {
                //     // 将ElementType类型属性值添加到table_vals_sql字符串中
                //     table_vals_sql.push_str(&format!(r#"'{}',"#, d.replace(r#"'"#, "")));
                // }
                // AttrVal::WordType(d) => {
                //     // 将WordType类型属性值添加到table_vals_sql字符串中
                //     table_vals_sql.push_str(&format!(r#"'{}',"#, d.replace(r#"'"#, "")));
                // }
                // AttrVal::RefU64Type(d) => {
                //     // 将RefU64Type类型属性值添加到table_vals_sql字符串中
                //     table_vals_sql.push_str(&format!("{},", d.0));
                // }
                // AttrVal::RefU64Array(d) => {
                //     // 将RefU64Array类型属性值序列化为JSON字符串后添加到table_vals_sql字符串中
                //     table_vals_sql.push_str(&format!(
                //         r#"'{}',"#,
                //         serde_json::to_string(d).unwrap_or_default()
                //     ));
                // }
                // AttrVal::StringHashType(_) => {}
                // }
            } else {
                // 如果udna没值，可能是在dyudna中
                if info_map.contains_key(&(*noun_hash as i32)) {
                    // 检查属性信息映射中是否存在属性哈希值对应的属性信息
                    let info = info_map.get(&(*noun_hash as i32)).unwrap();
                    match &info.default_val {
                        // 根据默认值的类型进行匹配
                        AttrVal::InvalidType => {}
                        AttrVal::IntegerType(d) => {
                            // 将整数类型的默认值添加到table_vals_sql字符串中
                            table_vals_sql.push_str(&format!("{},", d.to_string()));
                        }
                        AttrVal::StringType(d) => {
                            // 将字符串类型的默认值添加到table_vals_sql字符串中
                            table_vals_sql.push_str(&format!(r#"'{}',"#, d.replace(r#"'"#, "")));
                        }
                        AttrVal::DoubleType(d) => {
                            // 将浮点数类型的默认值添加到table_vals_sql字符串中（保留3位小数）
                            table_vals_sql.push_str(&format!("{},", f64_round_3(*d)));
                        }
                        AttrVal::DoubleArrayType(d) => {
                            // 将双精度浮点数数组类型的默认值序列化为字节数组，并将其转换为十六进制字符串后添加到table_vals_sql字符串中
                            table_vals_sql.push_str(&format!(
                                r#"0x{},"#,
                                hex::encode(bincode::serialize(d).unwrap_or_default().as_slice())
                            ));
                        }
                        AttrVal::StringArrayType(d) => {
                            // 将字符串数组类型的默认值序列化为JSON字符串后添加到table_vals_sql字符串中
                            table_vals_sql.push_str(&format!(
                                r#"'{}',"#,
                                serde_json::to_string(d).unwrap_or_default()
                            ));
                        }
                        AttrVal::BoolArrayType(d) => {
                            // 将布尔数组类型的默认值序列化为JSON字符串后添加到table_vals_sql字符串中
                            table_vals_sql.push_str(&format!(
                                r#"'{}',"#,
                                serde_json::to_string(d).unwrap_or_default()
                            ));
                        }
                        AttrVal::IntArrayType(d) => {
                            // 将整数数组类型的默认值序列化为JSON字符串后添加到table_vals_sql字符串中
                            table_vals_sql.push_str(&format!(
                                r#"'{}',"#,
                                serde_json::to_string(d).unwrap_or_default()
                            ));
                        }
                        AttrVal::BoolType(d) => {
                            // 将布尔类型的默认值转换为整数（1或0）后添加到table_vals_sql字符串中
                            let b = if *d { 1 } else { 0 };
                            table_vals_sql.push_str(&format!("{},", b));
                        }
                        AttrVal::Vec3Type(d) => {
                            // 将Vec3类型的默认值序列化为JSON字符串后添加到table_vals_sql字符串中
                            table_vals_sql.push_str(&format!(
                                r#"'{}',"#,
                                serde_json::to_string(d).unwrap_or_default()
                            ));
                        }
                        AttrVal::ElementType(d) => {
                            // 将ElementType类型的默认值添加到table_vals_sql字符串中
                            table_vals_sql.push_str(&format!(r#"'{}',"#, d.replace(r#"'"#, "")));
                        }
                        AttrVal::WordType(d) => {
                            // 将WordType类型的默认值添加到table_vals_sql字符串中
                            table_vals_sql.push_str(&format!(r#"'{}',"#, d.replace(r#"'"#, "")));
                        }
                        AttrVal::RefU64Type(d) => {
                            // 将RefU64Type类型的默认值添加到table_vals_sql字符串中
                            table_vals_sql.push_str(&format!("{},", d.0));
                        }
                        AttrVal::RefU64Array(d) => {
                            // 将RefU64Array类型的默认值序列化为JSON字符串后添加到table_vals_sql字符串中
                            table_vals_sql.push_str(&format!(
                                r#"'{}',"#,
                                serde_json::to_string(d).unwrap_or_default()
                            ));
                        }
                        AttrVal::StringHashType(_) => {}
                    }
                } else {
                    // 如果属性信息映射中不存在属性哈希值对应的属性信息，则将 "unset" 添加到table_vals_sql字符串中
                    table_vals_sql.push_str(r#"'unset',"#);
                }
            }
        }
    }
    table_vals_sql.remove(table_vals_sql.len() - 1);
    table_vals_sql.push_str("),");

    table_vals_sql
}

pub async fn save_to_arangodb_task() {}

pub async fn save_to_tidb_task() {}

pub async fn save_to_versioned_task() {}

///多线程同步数据，包括增量同步
pub async fn sync_total_async_threaded(
    // arango_pool: ArPool,
    db_option: &DbOption,
    project: &str,
    // pool: Pool<MySql>,
    // info_pool: Pool<MySql>,
    attmap_tree: sled::Tree,
    children_tree: sled::Tree,
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
    dbg!("解析uda文件");
    let _ = parse_uda_file(project, children_files.clone(), &need_parsed_files).await;
    // 正式解析
    let project = Arc::new(project.to_string()); // 创建一个Arc对象，表示项目名称
    let db_option = Arc::new(db_option.clone()); // 创建一个Arc对象，表示数据库选项
    let mut error_sql = Arc::new(DashSet::new()); // 创建一个Arc对象，表示错误的SQL集合
                                                  // 是否替换tidb的数据
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
        if file_name.ends_with("com") || file_name.ends_with("mis") {
            continue; // 如果文件名以 "com" 或 "mis" 结尾，则跳过循环
        }
        if only_sync_sys {
            if !file_name.ends_with("sys") {
                continue; // 如果仅同步系统表，并且文件名不以 "sys" 结尾，则跳过循环
            }
        }

        if need_parsed_files.is_none() || need_parsed_files.as_ref().unwrap().contains(&file_name) {
            // 如果需要解析的文件列表为空或包含当前文件名，则执行以下代码块
            println!("path={:?}", &file_name); // 打印文件路径
            let project_clone = project.clone(); // 创建项目名称的克隆
            let project_name = project.as_str().to_string(); // 获取项目名称的字符串
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
            let children_map_clone = Arc::new(children_map);

            if db_option.sync_graph_db.unwrap_or(true) {
                let arango_pool = connect_arangodb(&db_option).await?;
                let database = arango_pool
                    .get()
                    .await?
                    .db(&db_option.arangodb_database)
                    .await?; // 获取ArangoDB数据库连接
                save_pdms_level_edges_in_sync(&database, &children_map_clone).await?;
                // 同步pdms_level_edges到图数据库
            }

            // let versioned_client = Arc::new(get_versioned_client(&db_option.project_name).await);
            for (chunk_index, chunk_refnos) in all_refnos.chunks(chunk_size).enumerate() {
                //terminus 的方法
                // let versioned_client =
                //     Arc::new(get_versioned_client(&db_option.project_name).await);
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
                })
                .await
                {
                    println!("Processing {} chunk index: {chunk_index}", &file_name);

                    if sync_tidb {
                        let default_conn_str = AiosDBManager::get_default_conn_str(&db_option);
                        let info_pool = AiosDBManager::get_db_pool(
                            &default_conn_str,
                            &format!("{}_{}", PDMS_INFO_DB, &db_option.project_name),
                        )
                        .await?;

                        let mut dbinfo_value_sql = gen_dbinfo_value_insert_sql(
                            db_no,
                            &file_name,
                            version,
                            project_clone.clone().as_str(),
                            db_type.clone(),
                        );
                        // let mut info_conn = info_pool.acquire().await.unwrap();

                        //保存dbno的信息表
                        let mut sql = format!("REPLACE INTO {PDMS_DBNO_INFOS_TABLE} ( id, NUMBDB, FILENAME,VERSION,PROJECT,DB_TYPE ) VALUES ");
                        sql.push_str(dbinfo_value_sql.as_str());
                        if is_replace {
                            sql = sql.replace("INSERT IGNORE", "REPLACE");
                        }
                        let result = info_pool.execute(sql.as_str()).await;
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
                        let result = execute_sql(&info_pool, sql.as_str()).await;
                    }

                    // version_map.entry(file_name.clone()).or_insert(version);
                    // set_uda_attr(&type_ele_map, &total_attr_map, &mut uda_map)?;

                    // for kv in &type_ele_map {
                    //     let noun: i32 = *kv.key() as _;
                    //     let type_name = db1_dehash(noun as _);
                    //     // if type_name.as_str() != "SDTE" {
                    //     //     continue;
                    //     // }
                    //     dbg!((&type_name, noun));
                    //     dbg!(kv.value().len());
                    //     for refnos in &kv.value().iter().chunks(ATTS_CHUNK_COUNT) {
                    //         let mut maps = vec![];
                    //         for refno in refnos {
                    //             let att = total_attr_map.get(&refno).unwrap().merge();
                    //             let named_att_map: NamedAttrMap = att.into();
                    //             //需要检查一遍能否插入，不能就需要更新schema
                    //             if let Some(new_schema) = db_info.check_schema(noun, &named_att_map) {
                    //                 dbg!(&new_schema);
                    //                 let schema_res = client.insert_doc(new_schema.as_str(), "dpc", "Update schema", true, false, true).await?;
                    //                 dbg!(schema_res);
                    //             }
                    //             let map = named_att_map.gen_versioned_json_map();
                    //             maps.push(map);
                    //         }
                    //         // dbg!(&json);
                    //         let json = serde_json::to_string(&maps).unwrap_or_default();
                    //         let att_res = client.insert_doc(json.as_str(), "dpc", "Add Attributes", false, false, true).await?;
                    //         dbg!(att_res);
                    //         if !att_res {
                    //             let first = maps.iter().next();
                    //             dbg!(first);
                    //             dbg!(serde_json::to_string(&first));
                    //             // break;
                    //         }
                    //         break;
                    //     }

                    // }

                    //类型暂时不多线程
                    let total_attr_map_arc = Arc::new(total_attr_map);
                    let children_map_arc = children_map_clone.clone();
                    let mut type_handles = vec![];
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
                            // 单独保存plin
                            // save_plin_attr_arangodb(&database, &type_ele_map, &total_attr_map_arc)
                            //     .await?;
                            // 将 para 和 des_para保存的图数据库中
                            // save_paras_into_arangodb(&database, &total_attr_map_arc).await?;
                            // 将 dtse下的data部分数据保存到图数据库
                            save_dtse_value_to_arangodb(
                                &database,
                                &type_ele_map,
                                &total_attr_map_arc,
                            )
                            .await?;
                        }
                        println!("图数据库保存完成");
                    }

                    if db_option.sync_localdb.unwrap_or(true) {
                        for kv in total_attr_map_arc.as_ref() {
                            let att = kv.value().merge();
                            // let mut vec = att.into_rkyv_compress_bytes();
                            // // 将attmap_tree插入数据库
                            // attmap_tree.insert((**kv.key()).to_be_bytes().as_slice(), &*vec)?;
                        }
                    }
                    //开始执行保存数据
                    dbg!("开始保存pdms_element数据");
                    save_pdms_eles_to_versioned(
                        &db_option,
                        project.as_str(),
                        &total_attr_map_arc,
                        db_no as i32,
                        &children_map_clone,
                    )
                    .await?;
                    dbg!("开始保存属性数据");
                    const ATTS_CHUNK_COUNT: usize = 500;
                    for kv in type_ele_map.iter() {
                        let noun: i32 = *kv.key() as _;
                        let type_name = db1_dehash(noun as _);
                        if type_name.is_empty() {
                            continue;
                        }
                        for refnos in &kv.value().iter().chunks(ATTS_CHUNK_COUNT) {
                            let mut data_vec = vec![];
                            for refno in refnos {
                                let att: NamedAttrMap =
                                    total_attr_map_arc.get(refno).unwrap().merge().into();
                                data_vec.push(att.gen_versioned_json_map());
                            }
                            //使用surreal 保存NamedAttrMap
                            SUL_DB
                                .query(format!("INSERT IGNORE INTO {} $values", &type_name))
                                .bind(("values", &data_vec))
                                .await
                                .unwrap();

                            // NamedAttrMap::exec_insert_many(data_vec, &vdb, false).await.unwrap();
                            // break;
                        }
                        // break;
                    }

                    //如果不需要同步tidb，continue
                    if !sync_tidb {
                        continue;
                    }

                    for (type_hash, type_refnos) in type_ele_map {
                        if b_replace_types {
                            let replace_types = replace_types.clone().unwrap(); // 获取替换的类型列表
                            let att_type = db1_dehash(type_hash); // 获取当前类型的字符串表示
                            if !replace_types.contains(&att_type) {
                                continue; // 如果当前类型不在替换的类型列表中，则跳过循环
                            }
                        }
                        let total_attr_map_arc = total_attr_map_arc.clone();
                        let children_map_arc = children_map_arc.clone();
                        let default_conn_str = AiosDBManager::get_default_conn_str(&db_option);
                        let pool_clone =
                            AiosDBManager::get_db_pool(&default_conn_str, project_name.as_str())
                                .await?;
                        // let info_pool = AiosDBManager::get_db_pool(
                        //     &default_conn_str,
                        //     &format!("{}_{}", PDMS_INFO_DB, &db_option.project_name),
                        // ).await?;
                        // let pool_clone = pool.clone();
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
                                            let name = att
                                                .explicit_attmap
                                                .get_name_string()
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
        if !error_sql.is_empty() {
            // 重新执行有问题的sql
            println!("正在重新插入有问题的sql语句, 共 {} 条", error_sql.len());
            let mut conn = pool;
            for sql in error_sql.iter() {
                let _ = conn.execute(sql.key().as_str()).await;
                // if r.is_ok() {
                //     error_sql.remove(sql.key());
                // }
            }
        }
    }

    // 保存每个file最新的page_num
    // if version_map.len() > 0 {
    //     let table_sql = gen_create_file_version_table_sql();
    //     let mut version_sql =
    //         format!("INSERT IGNORE INTO {PDMS_FILE_VERSION_TABLE} (FILENAME,VERSION) VALUES");
    //     for (file_name, version) in version_map.into_iter() {
    //         version_sql.push_str(&format!("('{}',{}),", file_name, version))
    //     }
    //     let mut project_conn = pool.acquire().await.unwrap();
    //     version_sql.remove(version_sql.len() - 1);
    //     let result = project_conn.execute(table_sql.as_str()).await;
    //     match result {
    //         Ok(_) => {}
    //         Err(e) => {
    //             dbg!(&e);
    //             dbg!(table_sql.as_str());
    //         }
    //     }
    //     let result = project_conn.execute(version_sql.as_str()).await;
    //     match result {
    //         Ok(_) => {}
    //         Err(e) => {
    //             dbg!(&e);
    //             dbg!(version_sql.as_str());
    //         }
    //     }
    // }

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
            // 若udna中没有值，则可能在显式属性的dyudna中
            let mut udna = uda_implicit_att.get_str("UDNA");
            if udna == Some("") {
                udna = uda_explicit_att.get_str("DYUDNA");
            }
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
