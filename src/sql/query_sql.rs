use std::collections::BTreeMap;
use aios_core::pdms_types::{AiosStr, AttrInfo, AttrMap, AttrVal, DbAttributeType, NounHash, RefU64};
use anyhow::anyhow;
use dashmap::DashMap;
use parse_pdms_db::db1_dehash;
use parse_pdms_db::db_tool::db1_hash;
use smol_str::SmolStr;
use sqlx::{MySql, Pool, Row};
use crate::api::{attr, element};
use crate::api::element::{query_children, query_pdms_elements_type_name, query_refno_infos};
use crate::database::get_tidb_pool;
use crate::REFNO_INFO_MAP;
use crate::sql::gen_sql::*;

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
    let v = attr::query_explicit_attr(refno, pool).await?;
    println!("v={:?}", v.to_string_hashmap());
    Ok(())
}

#[tokio::test]
async fn test_query_full_attr() -> anyhow::Result<()> {
    let url = "mysql://root:root@127.0.0.1:3306";
    let info_pool = get_tidb_pool(&format!("{}/{}", url, "refno_infos")).await;
    let refno = RefU64(65721589565564);
    let project = query_refno_infos(refno, info_pool).await?;
    let pool = get_tidb_pool(&format!("{}/{}", url, project)).await;
    let v = attr::query_full_attr(refno, pool).await?;
    println!("v={:?}", v.to_string_hashmap());
    Ok(())
}