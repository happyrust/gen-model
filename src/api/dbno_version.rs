use sqlx::{Error, MySql, Pool, Row};
use sqlx::mysql::MySqlRow;

// 获取dbno的版本号
pub async fn query_dbno_version(dbno:i32,pool:&Pool<MySql>) -> anyhow::Result<Option<i32>> {
    let sql = gen_query_dbno_version_sql(dbno);
    let result = sqlx::query(&sql).fetch_one(pool).await;
    match result {
        Ok(val) => {
            let version = val.get::<i32,_>("VERSION");
            return Ok(Some(version));
        }
        Err(e) => {
            dbg!(&e);
            dbg!(&sql);
        }
    }
    Ok(None)
}

fn gen_query_dbno_version_sql(dbno:i32) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT VERSION , PROJECT ,DB_TYPE FROM DBNO_INFOS WHERE NUMBDB = {}",dbno));
    sql
}