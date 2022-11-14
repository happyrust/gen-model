use std::collections::HashMap;
use std::env;
use aios_core::pdms_types::RefU64;
use sqlx::{MySql, Pool, Row};
use sqlx::Executor;
use anyhow::Result;
use dashmap::DashMap;
use futures::poll;
use lazy_static::lazy_static;
use crate::api::children::query_numbdb_by_refno;
use crate::consts::*;
use crate::api::element::{query_mdb_module_worlds, query_mdb_module_worlds_fix};
use crate::data_interface::tidb_manager::AiosDBManager;

lazy_static! {
    pub static ref MDB_MODULE_NUMBDBS: Vec<i32> = {
        let mut result = vec![];
        result
    };
}

pub async fn insert_project_mdb(project_name:&str,pool: &Pool<MySql>, info_pool: &Pool<MySql>) -> anyhow::Result<()> {
    // let project_mdb = query_mdb_module_worlds(pool, info_pool).await?;
    let project_mdb = query_mdb_module_worlds_fix(project_name,pool, info_pool).await?;
    let project_mdb_len = project_mdb.len();
    let sql = gen_insert_project_mdb_sql(project_mdb.clone());
    let json_sql = gen_insert_project_mdb_json_sql(project_mdb);
    if project_mdb_len != 0 {
        let mut conn = pool.acquire().await?;
        let result = conn.execute(sql.as_str()).await;
        match result {
            Ok(_) => {}
            Err(e) => {
                dbg!(&e);
                dbg!(sql.as_str());
            }
        }
        let json_result = conn.execute(json_sql.as_str()).await;
        match json_result {
            Ok(_) => {}
            Err(e) => {
                dbg!(&e);
                dbg!(json_sql.as_str());
            }
        }
    }
    Ok(())
}

pub async fn query_world_data(mdb: &str, module: &str, pool: &Pool<MySql>) -> anyhow::Result<Vec<u8>> {
    let sql = gen_query_world_sql(mdb, module);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    Ok(result.get::<Vec<u8>, _>(0))
}

/// 查询 mdb 和module 包含了哪些 numdb
pub async fn query_mdb_contain_numbdb(mdb: &str, module: &str, pool: &Pool<MySql>) -> anyhow::Result<Vec<i32>> {
    let mut r = vec![];
    let numbdb_worlds = query_world_data(mdb, module, pool).await?;
    let refnos = bincode::deserialize::<Vec<RefU64>>(&numbdb_worlds)?;
    for refno in refnos {
        let numbdb = query_numbdb_by_refno(refno, pool).await?;
        r.push(numbdb);
    }
    Ok(r)
}

pub fn gen_insert_project_mdb_sql(mdbs: HashMap<String, HashMap<String, Vec<RefU64>>>) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("INSERT IGNORE INTO {PDMS_PROJECT_MDB_TABLE} (MDB_NAME,DB_TYPE,DATA) VALUES "));
    for (name, vals) in mdbs {
        for (db_type, data) in vals {
            let data = hex::encode(bincode::serialize(&data).unwrap());
            sql.push_str(&format!("( '{}' , '{}', 0x{} ),", &name, db_type, data));
        }
    }
    sql.remove(sql.len() - 1);
    sql
}

pub fn gen_insert_project_mdb_json_sql(mdbs: HashMap<String, HashMap<String, Vec<RefU64>>>) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("INSERT IGNORE INTO {PDMS_PROJECT_MDB_TABLE_JSON} (MDB_NAME,DB_TYPE,DATA) VALUES "));
    for (name, vals) in mdbs {
        for (db_type, data) in vals {
            let data = serde_json::to_string(&data).unwrap();
            sql.push_str(&format!("( '{}' , '{}', '{}' ),", &name, db_type, data));
        }
    }
    sql.remove(sql.len() - 1);
    sql
}


fn gen_query_world_sql(mdb: &str, module: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT DATA FROM {PDMS_PROJECT_MDB_TABLE} WHERE MDB_NAME = '{}' and db_type = '{}' ;", mdb, module));
    sql
}

#[tokio::test]
async fn test_query_mdb_contain_numbdb() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let numbdbs = query_mdb_contain_numbdb("/SAMPLE", "DESI", &pool).await?;
    println!("{:?}", numbdbs);
    Ok(())
}