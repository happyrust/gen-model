use crate::consts::*;
use aios_core::pdms_types::RefU64;
use sqlx::mysql::MySqlRow;
use sqlx::{Error, MySql, Pool, Row};

pub async fn query_dbno_count(dbno: i32, pool: &Pool<MySql>, project: &str) -> anyhow::Result<i32> {
    let sql = gen_query_dbno_count(dbno, project);
    let result = sqlx::query(&sql).fetch_one(pool).await?;
    Ok(result.try_get::<i32, _>(0)?)
}

fn gen_query_dbno_count(dbno: i32, project: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!(
        "SELECT COUNT(*) FROM {PDMS_DBNO_INFOS_TABLE} WHERE NUMBDB = {} AND PROJECT = '{}'",
        dbno, project
    ));
    sql
}
