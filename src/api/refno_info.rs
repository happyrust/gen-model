use std::collections::HashMap;
use sqlx::{Error, MySql, Pool, Row};
use sqlx::mysql::MySqlRow;

pub async fn get_refno_infos(pool: Pool<MySql>) -> anyhow::Result<HashMap<i32, String>> {
    let mut map = HashMap::new();
    let sql = "select ref0 , project from refno_infos";
    let results = sqlx::query(sql).fetch_all(&mut pool.acquire().await?).await;
    match results {
        Ok(vals) => {
            for val in vals {
                let dbno = val.get::<i32, _>("ref0");
                let project = val.get::<String, _>("project");
                map.entry(dbno).or_insert(project);
            }
        }
        Err(e) => {
            dbg!(e);
            dbg!(sql);
        }
    }
    Ok(map)
}