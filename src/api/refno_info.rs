use std::collections::HashMap;
use std::sync::Arc;
use aios_core::db_number::DbNumMgr;
use aios_core::pdms_types::{NounHash, RefU64};
use anyhow::anyhow;
use crate::consts::*;
use dashmap::DashMap;
use sqlx::{Error, MySql, Pool, Row};
use sqlx::mysql::MySqlRow;
use crate::data_interface::cache::CACHED_REFNO_BASIC_MAP;
use crate::data_interface::defines::CachedRefBasic;
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

/// 获取生成refno到RefBasic的映射
pub async fn sync_refno_basic_map(pool: &Pool<MySql>, dbno_mgr: &mut DbNumMgr) -> anyhow::Result<bool> {
    let sql = format!("SELECT ID, OWNER, TYPE, NUMBDB  FROM {PDMS_ELEMENTS_TABLE}");
    let results = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await;
    match results {
        Ok(vals) => {
            for val in vals {
                let refno = (val.get::<i64, _>("ID") as u64).into();
                let owner = (val.get::<i64, _>("OWNER") as u64).into();
                let type_name = val.get::<String, _>("TYPE");
                let dbno = val.get::<i32, _>("NUMBDB");
                dbno_mgr.insert(refno, dbno);
                let table = qualified_table_name(type_name.as_str());
                CACHED_REFNO_BASIC_MAP.insert(refno, CachedRefBasic {
                    owner,
                    table,
                });
            }
        }
        Err(e) => {
            dbg!(&e);
            dbg!(sql);
            return Err(anyhow!(e.to_string()));
        }
    }
    Ok(true)
}