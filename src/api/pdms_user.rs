use std::env;
use dashmap::DashSet;
use sqlx::{MySql, Pool, Row};
use crate::api::project_mdb::query_db_nums_of_mdb;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::consts::PDMS_ELEMENTS_TABLE;

/// 传入一个用户名，判断是否存在与 pdms 的用户中
pub async fn query_b_existence_pdms_user(user_name:&str,pool:&Pool<MySql>) -> anyhow::Result<bool> {
    let sql = gen_query_b_existence_pdms_user_sql(user_name);
    let val = sqlx::query(&sql).fetch_one(pool).await;
    if let Ok(val) = val {
        let b_exist = val.get::<i32,_>("COUNT(1)");
        return if b_exist > 0 { Ok(true) } else { Ok(false) }
    }
    Ok(false)
}

pub async fn query_all_pdms_user(pool:&Pool<MySql>) -> anyhow::Result<DashSet<String>> {
    let mut result = DashSet::new();
    let sql = gen_query_all_user_sql();
    let val = sqlx::query(&sql).fetch_all(pool).await;
    if let Ok(vals) = val {
        for val in vals {
            let name = val.get::<String,_>("NAME");
            result.insert(name);
        }
    }
    Ok(result)
}

fn gen_query_b_existence_pdms_user_sql(user_name:&str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT COUNT(1) FROM {PDMS_ELEMENTS_TABLE} WHERE TYPE = 'USER' AND NAME = '{}'",user_name));
    sql
}

fn gen_query_all_user_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT NAME FROM {PDMS_ELEMENTS_TABLE} WHERE TYPE = 'USER'"));
    sql
}


#[tokio::test]
async fn test_query_b_existence_pdms_user() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let numbdbs = query_b_existence_pdms_user("SYSTEM", &pool).await?;
    println!("{:?}", numbdbs);
    Ok(())
}

#[tokio::test]
async fn test_query_all_pdms_user() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let numbdbs = query_all_pdms_user( &pool).await?;
    println!("{:?}", numbdbs);
    Ok(())
}