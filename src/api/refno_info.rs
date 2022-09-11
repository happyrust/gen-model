use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use aios_core::cache::refno::CachedRefBasic;
use aios_core::db_number::DbNumMgr;
use aios_core::pdms_types::{NounHash, RefU64, RefU64Vec};
use anyhow::anyhow;
use arangors_lite::Database;
use crate::consts::*;
use dashmap::DashMap;
use sqlx::{Error, MySql, Pool, Row};
use sqlx::mysql::MySqlRow;
use crate::api::element::query_types_refnos;
use crate::aql_api::plin_attr::query_plin_attrs;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::helper::qualified_table_name;
use crate::defines::CACHED_REFNO_BASIC_MAP;

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

pub async fn cache_plin_plax(pool: &Pool<MySql>, dbnos: Option<Vec<i32>>, database: &Database) -> anyhow::Result<DashMap<RefU64, String>> {
    let mut fitt_map = vec![];
    let fitt_refnos = query_types_refnos(&vec!["FITT"], pool, dbnos).await?;
    if fitt_refnos.len() == 0 { return Ok(DashMap::new()); }
    let sql = gen_query_refnos_implicit_string_attr("FITT", vec!["POSL"], fitt_refnos);
    let results = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for result in results {
        let refno = RefU64(result.get::<i64, _>("ID") as u64);
        let pos_line = result.get::<String, _>("POSL");
        fitt_map.push((refno, pos_line));
    }
    let result = query_plin_attrs(fitt_map, database).await.unwrap_or(DashMap::new());
    Ok(result)
}

fn gen_query_refnos_implicit_string_attr(table_name: &str, value: Vec<&str>, refnos: RefU64Vec) -> String {
    let mut filed = String::from("ID ,".to_string());
    for v in value {
        filed.push_str(&format!("{} ,", v))
    }
    filed.remove(filed.len() - 1);

    let mut refno_strs = String::new();
    for refno in refnos {
        refno_strs.push_str(&format!("{} ,", refno.0));
    }
    refno_strs.remove(refno_strs.len() - 1);

    let mut sql = String::new();
    sql.push_str(&format!("SELECT {} FROM {table_name} WHERE ID IN ( {} )", filed, refno_strs));
    sql
}