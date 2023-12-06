use std::collections::HashMap;
use std::env;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use aios_core::pdms_types::RefU64;
use anyhow::Result;
use dashmap::DashMap;
use futures::poll;
use lazy_static::lazy_static;
use parry3d::utils::hashmap::FxHasher32;
use sqlx::{MySql, Pool, Row};
use sqlx::Executor;
use crate::api::element::{DbQuickInfo, MdbQuickInfoMap};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::consts::*;

lazy_static! {
    pub static ref MDB_MODULE_NUMBDBS: Vec<i32> = {
        let mut result = vec![];
        result
    };
}

//(ID, DB_NUM, MDB_NAME, REFNO, PROJECT, WORLD_REFNO, DB_TYPE)
pub async fn query_db_quick_info(mdb: &str, module: &str, pool: &Pool<MySql>) -> anyhow::Result<Vec<DbQuickInfo>> {
    let mut sql = String::new();
    let mdb = if mdb.starts_with("/") { mdb.to_string() } else { format!("/{}", mdb) };
    sql.push_str(&format!("SELECT * FROM {PDMS_PROJECT_MDB_TABLE} WHERE MDB_NAME = '{}' and db_type = '{}' ORDER BY ORDER_NUM ;", mdb, module));
    let result = sqlx::query(&sql).fetch_all(pool).await?;
    let mut vec = vec![];
    for r in result {
        vec.push(DbQuickInfo{
            refno: RefU64::from_str(&r.get::<String, _>("REFNO")).unwrap(),
            world_refno: RefU64::from_str(&r.get::<String, _>("WORLD_REFNO")).unwrap(),
            db_num: r.get::<i32, _>("DB_NUM"),
            db_type: r.get::<String, _>("DB_TYPE"),
            project: r.get::<String, _>("PROJECT"),
            order_number: r.get::<i32, _>("ORDER_NUM"),
        } );
    }
    Ok(vec)
}


pub async fn query_world_refnos(mdb: &str, module: &str, pool: &Pool<MySql>) -> anyhow::Result<Vec<RefU64>> {
    let mut sql = String::new();
    let mdb = if mdb.starts_with("/") { mdb.to_string() } else { format!("/{}", mdb) };
    sql.push_str(&format!("SELECT WORLD_REFNO FROM {PDMS_PROJECT_MDB_TABLE} WHERE MDB_NAME = '{}' and db_type = '{}' ORDER BY ORDER_NUM;", mdb, module));
    let result = sqlx::query(&sql).fetch_all(pool).await?;
    let mut vec = vec![];
    for r in result {
        vec.push(RefU64::from_str(&r.get::<String, _>(0)).unwrap() );
    }
    Ok(vec)
}

/// 查询 mdb 和module 包含了哪些 numdb
pub async fn query_db_nums_of_mdb(mdb: &str, module: &str, pool: &Pool<MySql>) -> anyhow::Result<Vec<i32>> {
    let mut sql = String::new();
    let mdb = if mdb.starts_with("/") { mdb.to_string() } else { format!("/{}", mdb) };
    sql.push_str(&format!("SELECT DB_NUM FROM {PDMS_PROJECT_MDB_TABLE} WHERE MDB_NAME = '{}' and db_type = '{}' ORDER BY ORDER_NUM;", mdb, module));
    let result = sqlx::query(&sql).fetch_all(pool).await?;
    let mut vec = vec![];
    for r in result {
        vec.push(r.get::<i32, _>(0));
    }
    Ok(vec)
}

pub async fn query_if_contains_mdb(mdb: &str, module: &str, pool: &Pool<MySql>) -> anyhow::Result<bool> {
    let sql = gen_query_contains_mdb_sql(mdb, module);
    let result = sqlx::query(&sql).fetch_one(pool).await?;
    let count = result.get::<i32, _>(0);
    Ok(count != 0)
}

pub fn gen_insert_project_mdb_sql(map: &MdbQuickInfoMap) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("REPLACE INTO {PDMS_PROJECT_MDB_TABLE} (ID, DB_NUM, MDB_NAME, REFNO, PROJECT, WORLD_REFNO, DB_TYPE, ORDER_NUM) VALUES "));
    for (name, vals) in map {
        for (db_type, data) in vals {
            for d in data {
                let mut s: FxHasher32 = Default::default();
                name.hash(&mut s);
                d.db_num.hash(&mut s);
                let id = s.finish();
                sql.push_str(&format!("({}, {} , '{}', '{}', '{}', '{}', '{}', {}),",
                                      id , d.db_num, name, &d.refno.to_string(), &d.project, &d.world_refno.to_string(), db_type, d.order_number));
            }
        }
    }
    sql.remove(sql.len() - 1);
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