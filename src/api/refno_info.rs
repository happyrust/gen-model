use std::collections::HashMap;
use aios_core::pdms_types::{NounHash, RefU64};
use crate::consts::*;
use dashmap::DashMap;
use sqlx::{Error, MySql, Pool, Row};
use sqlx::mysql::MySqlRow;
use crate::helper::qualified_table_name;

///更新获得ref0->project 缓存
pub async fn get_ref0_map(pool: &Pool<MySql>) -> anyhow::Result<DashMap<u32, String>> {
    let mut map = DashMap::new();
    let sql = "SELECT REF0 , PROJECT FROM REFNO_INFOS";
    let results = sqlx::query(sql).fetch_all(&mut pool.acquire().await?).await;
    match results {
        Ok(vals) => {
            for val in vals {
                let ref0 = val.get::<i32, _>("REF0") as u32;
                let project = val.get::<String, _>("PROJECT");
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

//获取生成refno到table name的映射
pub async fn get_refno_table_map(pool: &Pool<MySql>) -> anyhow::Result<DashMap<RefU64, String>> {
    let mut map = DashMap::new();
    let sql = format!("SELECT ID, TYPE  FROM {PDMS_ELEMENTS_TABLE}");
    let results = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await;
    match results {
        Ok(vals) => {
            for val in vals {
                let refno = (val.get::<i64, _>("ID") as u64).into();
                let type_name = val.get::<String, _>("TYPE");
                let table_name = qualified_table_name(type_name.as_str());
                map.insert(refno, table_name);
            }
        }
        Err(e) => {
            dbg!(e);
            dbg!(sql);
        }
    }
    Ok(map)
}