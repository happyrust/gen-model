use std::collections::HashMap;
use aios_core::pdms_types::RefU64;
use sqlx::{MySql, Pool, Row};
use sqlx::Executor;
use anyhow::Result;
use futures::poll;
use crate::consts::*;
use crate::api::element::query_mdb_module_worlds;

pub async fn insert_project_mdb(pool: &Pool<MySql>, info_pool: &Pool<MySql>) -> anyhow::Result<()> {
    let project_mdb = query_mdb_module_worlds(pool, info_pool).await?;
    dbg!(&project_mdb);
    let project_mdb_len = project_mdb.len().clone();
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