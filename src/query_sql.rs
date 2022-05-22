use std::collections::BTreeMap;
use std::env;
use aios_core::pdms_types::{AiosStr, AttrMap, EleNode, RefU64};
use crate::db_types::EleNodeTIDB;
use parse_pdms_db::db_tool::db1_hash;
use smol_str::SmolStr;
use sqlx::{MySql, Pool, Row};
use crate::api::{element};
use crate::api::element::query_refno_type;
use crate::consts::PDMS_INFO_DB;
use crate::data_interface::tidb_manager::AiosDBManager;


#[tokio::test]
async fn test_get_world() -> anyhow::Result<()> {
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool( &url, "sample").await?;
    let v = element::query_world("SAMPLE","DESI", pool).await?;
    println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_get_children() -> anyhow::Result<()> {
    let url = env::var("DATABASE_URL")?;
    let info_pool = AiosDBManager::get_db_pool( &url, PDMS_INFO_DB).await?;
    let refno = RefU64(65721589565564);
    let project = element::query_project_name(refno, info_pool).await?;
    let pool = AiosDBManager::get_db_pool( &url, &project).await?;
    let v = element::query_children(refno, pool).await?;
    println!("v={:?}", v);
    Ok(())
}