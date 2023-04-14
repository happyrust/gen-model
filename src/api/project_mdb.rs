use std::collections::HashMap;
use std::env;

use aios_core::pdms_types::RefU64;
use anyhow::Result;
use dashmap::DashMap;
use futures::poll;
use lazy_static::lazy_static;
use sqlx::{MySql, Pool, Row};
use sqlx::Executor;

use crate::api::children::query_db_num_by_refno;
use crate::api::element::{MdbQuickInfoMap};
use crate::consts::*;
use crate::data_interface::tidb_manager::AiosDBManager;

lazy_static! {
    pub static ref MDB_MODULE_NUMBDBS: Vec<i32> = {
        let mut result = vec![];
        result
    };
}

/// save project mdb info to database
// pub async fn insert_project_mdb(project_pool: &Pool<MySql>, info_pool: &Pool<MySql>) -> anyhow::Result<()> {
//     let project_mdb = query_mdb_module_world_refnos(project_pool, info_pool).await?;
//     dbg!(&project_mdb);
//     let project_mdb_len = project_mdb.len();
//     let sql = gen_insert_project_mdb_sql(project_mdb.clone());
//     let json_sql = gen_insert_project_mdb_json_sql(project_mdb);
//     if project_mdb_len != 0 {
//         let mut conn = project_pool.acquire().await?;
//         let result = conn.execute(sql.as_str()).await;
//         match result {
//             Ok(_) => {}
//             Err(e) => {
//                 dbg!(&e);
//                 dbg!(sql.as_str());
//             }
//         }
//         let json_result = conn.execute(json_sql.as_str()).await;
//         match json_result {
//             Ok(_) => {}
//             Err(e) => {
//                 dbg!(&e);
//                 dbg!(json_sql.as_str());
//             }
//         }
//     }
//     Ok(())
// }

pub async fn query_world_refnos(mdb: &str, module: &str, pool: &Pool<MySql>) -> anyhow::Result<Vec<RefU64>> {
    let mut sql = String::new();
    let mdb = if mdb.starts_with("/") { mdb.to_string() } else { format!("/{}", mdb) };
    sql.push_str(&format!("SELECT WORLD_REFNO FROM {PDMS_PROJECT_MDB_TABLE} WHERE MDB_NAME = '{}' and db_type = '{}' ;", mdb, module));
    let result = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    let mut vec = vec![];
    for r in result {
        vec.push(RefU64::from_refno_str(&r.get::<String, _>(0)).unwrap() );
    }
    Ok(vec)
}

/// 查询 mdb 和module 包含了哪些 numdb
pub async fn query_db_nums_of_mdb(mdb: &str, module: &str, pool: &Pool<MySql>) -> anyhow::Result<Vec<i32>> {
    let mut sql = String::new();
    let mdb = if mdb.starts_with("/") { mdb.to_string() } else { format!("/{}", mdb) };
    sql.push_str(&format!("SELECT DB_NUM FROM {PDMS_PROJECT_MDB_TABLE} WHERE MDB_NAME = '{}' and db_type = '{}' ;", mdb, module));
    let result = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    let mut vec = vec![];
    for r in result {
        vec.push(r.get::<i32, _>(0));
    }
    Ok(vec)
}

pub async fn query_if_contains_mdb(mdb: &str, module: &str, pool: &Pool<MySql>) -> anyhow::Result<bool> {
    let sql = gen_query_contains_mdb_sql(mdb, module);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    let count = result.get::<i32, _>(0);
    Ok(count != 0)
}

pub fn gen_insert_project_mdb_sql(map: &MdbQuickInfoMap) -> String {
    let mut sql = String::new();
    // sql.push_str(&format!(r#"{} PRIMARY INT,"#, "DB_NUM"));
    // sql.push_str(&format!(r#"{} VARCHAR(20) ,"#, "MDB_NAME"));
    // sql.push_str(&format!(r#"{} VARCHAR(30) ,"#, "REFNO"));
    // sql.push_str(&format!(r#"{} VARCHAR(30) ,"#, "PROJECT"));
    // sql.push_str(&format!(r#"{} VARCHAR(30) ,"#, "WORLD_REFNO"));
    // sql.push_str(&format!(r#"{} VARCHAR(10) ,"#, "DB_TYPE"));
    sql.push_str(&format!("REPLACE IGNORE INTO {PDMS_PROJECT_MDB_TABLE} (DB_NUM, MDB_NAME, REFNO, PROJECT, WORLD_REFNO, DB_TYPE) VALUES "));
    for (name, vals) in map {
        for (db_type, data) in vals {
            for d in data {
                sql.push_str(&format!("( {} , '{}', '{}', '{}', '{}', '{}' ),",
                                      d.db_num, name, &d.refno.to_string(), &d.project, &d.world_refno.to_string(), db_type));
            }
        }
    }
    sql.remove(sql.len() - 1);
    sql
}

// pub fn gen_insert_project_mdb_json_sql(mdbs: &MdbQuickInfoMap) -> String {
//     let mut sql = String::new();
//     sql.push_str(&format!("INSERT IGNORE INTO {PDMS_PROJECT_MDB_TABLE_JSON} (MDB_NAME,DB_TYPE,DATA) VALUES "));
//     for (name, vals) in mdbs {
//         for (db_type, data) in vals {
//             let data = serde_json::to_string(&data).unwrap();
//             sql.push_str(&format!("( '{}' , '{}', '{}' ),", &name, db_type, data));
//         }
//     }
//     sql.remove(sql.len() - 1);
//     sql
// }


fn gen_query_world_sql(mdb: &str, module: &str) -> String {
    let mut sql = String::new();
    let mdb = if mdb.starts_with("/") { mdb.to_string() } else { format!("/{}", mdb) };
    sql.push_str(&format!("SELECT DATA FROM {PDMS_PROJECT_MDB_TABLE} WHERE MDB_NAME = '{}' and db_type = '{}' ;", mdb, module));
    sql
}

fn gen_query_contains_mdb_sql(mdb: &str, module: &str) -> String {
    let mut sql = String::new();
    let mdb = if mdb.starts_with("/") { mdb.to_string() } else { format!("/{}", mdb) };
    sql.push_str(&format!("SELECT COUNT(1) FROM {PDMS_PROJECT_MDB_TABLE} WHERE MDB_NAME = '{}' and db_type = '{}' ;", mdb, module));
    sql
}

#[tokio::test]
async fn test_query_mdb_contain_numbdb() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let numbdbs = query_db_nums_of_mdb("/SAMPLE", "DESI", &pool).await?;
    println!("{:?}", numbdbs);
    Ok(())
}