use sqlx::{Error, MySql, Pool, Row};
use sqlx::mysql::MySqlRow;
use crate::consts::*;

pub async fn query_dbtype_from_dbno(dbno: i32, project: &str, pool: &Pool<MySql>) -> anyhow::Result<Option<String>> {
    let sql = gen_query_dbtype_from_dbno(dbno, project);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    return match result {
        Ok(v) => { Ok(Some(v.get::<String, _>(0))) }
        Err(_) => { Ok(None) }
    };
}

fn gen_query_dbtype_from_dbno(dbno: i32, project: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT DB_TYPE FROM {PDMS_DBNO_INFOS_TABLE} WHERE NUMBDB = {} AND PROJECT = {} ", dbno, project));
    sql
}

pub async fn query_dbtype_from_dbno_count(dbno: i32, pool: &Pool<MySql>, project: &str) -> anyhow::Result<i32> {
    let sql = gen_query_dbtype_from_dbno_count(dbno, project);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    Ok(result.try_get::<i32, _>(0)?)
}

fn gen_query_dbtype_from_dbno_count(dbno: i32, project: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT COUNT(*) FROM {PDMS_DBNO_INFOS_TABLE} WHERE NUMBDB = {} AND PROJECT = {}", dbno, project));
    sql
}