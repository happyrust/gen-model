use std::collections::BTreeMap;
use aios_core::pdms_types::{AiosStr, AttrInfo, AttrMap, AttrVal, DbAttributeType, NounHash, RefU64};
use anyhow::anyhow;
use dashmap::DashMap;
use parse_pdms_db::db1_dehash;
use parse_pdms_db::db_tool::db1_hash;
use smol_str::SmolStr;
use sqlx::{MySql, Pool, Row};
use crate::api::{element, attr};
use crate::database::get_tidb_pool;
use crate::query_sql::{query_children, query_pdms_elements_type_name, query_refno_infos};
use crate::REFNO_INFO_MAP;
use crate::sql::gen_sql::*;

pub async fn query_world_children(pool:Pool<MySql>) -> anyhow::Result<Vec<(RefU64,AiosStr)>> {
    let mut b_map = BTreeMap::new();
    let sql = gen_query_type_refnos_sql("WORL");
    // 找到所有的world
    let result = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    let mut v = vec![];
    for r in result {
        let refno = r.get::<i64,_>(0);
        // 找到所有的world 对应的children
        let children = query_children(RefU64(refno as u64),pool.clone()).await?;
        b_map.insert(refno,children);
    }
    for (_,val) in b_map  {
        v.push(val);
    }
    Ok(v.into_iter().flatten().collect::<Vec<(RefU64,AiosStr)>>())
}

pub async fn query_explicit_attr(refno:RefU64,pool:Pool<MySql>) -> anyhow::Result<AttrMap> {
    let sql = gen_query_explicit_attr_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    let val = result.get::<Vec<u8>,_>("data");
    Ok(bincode::deserialize::<AttrMap>(&val)?)
}

pub async fn query_all_attr(refno:RefU64,pool:Pool<MySql>) -> anyhow::Result<()> {
    let implicit_attr = attr::query_implicit_attr(refno, pool.clone());
    Ok(())
}


#[tokio::test]
async fn test_query_implicit_attr() -> anyhow::Result<()> {
    let url = "mysql://root:root@127.0.0.1:3306";
    let info_pool = get_tidb_pool(&format!("{}/{}", url, "refno_infos")).await;
    let refno = RefU64(103010495627266);
    let project = query_refno_infos(refno, info_pool).await?;
    let pool = get_tidb_pool(&format!("{}/{}", url, project)).await;
    let v = attr::query_implicit_attr(refno, pool).await.unwrap();
    println!("v={:?}", v.to_string_hashmap());
    Ok(())
}

#[tokio::test]
async fn test_get_world_children() -> anyhow::Result<()> {
    let url = "mysql://root:root@127.0.0.1:3306";
    let info_pool = get_tidb_pool(&format!("{}/{}", url, "refno_infos")).await;
    let refno = RefU64(66108136620032);
    let project = query_refno_infos(refno, info_pool).await?;
    let pool = get_tidb_pool(&format!("{}/{}", url, project)).await;
    let v = element::query_children_pdms_tree(refno, pool).await?;
    println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_query_explicit_attr() -> anyhow::Result<()> {
    let url = "mysql://root:root@127.0.0.1:3306";
    let info_pool = get_tidb_pool(&format!("{}/{}", url, "refno_infos")).await;
    let refno = RefU64(105548821299733);
    let project = query_refno_infos(refno, info_pool).await?;
    let pool = get_tidb_pool(&format!("{}/{}", url, project)).await;
    let v = query_explicit_attr( refno,pool).await?;
    println!("v={:?}", v.to_string_hashmap());
    Ok(())
}