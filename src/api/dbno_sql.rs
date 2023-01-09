use sqlx::{Error, MySql, Pool, Row};
use sqlx::mysql::MySqlRow;
use crate::consts::*;

/// 获得dbtype
pub async fn query_dbtype_from_dbno(dbno: i32, pool: &Pool<MySql>, project: &str) -> anyhow::Result<Option<String>> {
    let mut sql = String::new();
    //todo 参考号相同的情况，导致refno获取出来的不准
    // sql.push_str(&format!(r#"SELECT DB_TYPE FROM {PDMS_DBNO_INFOS_TABLE} WHERE NUMBDB = {} AND PROJECT = '{}'"#, dbno, project));
    sql.push_str(&format!(r#"SELECT DB_TYPE FROM {PDMS_DBNO_INFOS_TABLE} WHERE NUMBDB = {}"#, dbno));
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    return match result {
        Ok(v) => { Ok(Some(v.get::<String, _>(0))) }
        Err(_) => {
            dbg!(&sql);
            Ok(None)
        }
    };
}



pub async fn query_dbno_count(dbno: i32, pool: &Pool<MySql>, project: &str) -> anyhow::Result<i32> {
    let sql = gen_query_dbno_count(dbno, project);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    Ok(result.try_get::<i32, _>(0)?)
}

fn gen_query_dbno_count(dbno: i32, project: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT COUNT(*) FROM {PDMS_DBNO_INFOS_TABLE} WHERE NUMBDB = {} AND PROJECT = '{}'", dbno, project));
    sql
}