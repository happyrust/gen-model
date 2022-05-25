use std::collections::HashMap;
use dashmap::DashMap;
use sqlx::{Error, MySql, Pool, Row};
use sqlx::mysql::MySqlRow;

///更新获得ref0->project 缓存
pub async fn get_refno_infos(pool: &Pool<MySql>) -> anyhow::Result<DashMap<u32, String>> {
    let mut map = DashMap::new();
    let sql = "select ref0 , project from refno_infos";
    let results = sqlx::query(sql).fetch_all(&mut pool.acquire().await?).await;
    match results {
        Ok(vals) => {
            for val in vals {
                let ref0 = val.get::<i32, _>("ref0") as u32;
                let project = val.get::<String, _>("project");
                map.entry(ref0).or_insert(project);
            }
        }
        Err(e) => {
            dbg!(e);
            dbg!(sql);
        }
    }
    Ok(map)
}