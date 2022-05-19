use std::collections::BTreeMap;
use aios_core::pdms_types::{AiosStr, AttrMap, EleNode, RefU64};
use crate::db_types::EleNodeTIDB;
use parse_pdms_db::db_tool::db1_hash;
use smol_str::SmolStr;
use sqlx::{MySql, Pool, Row};
use crate::api::{element};
use crate::database::get_tidb_pool;
use crate::api::element::query_refno_type;


#[tokio::test]
async fn test_get_world() -> anyhow::Result<()> {
    let url = "mysql://root:root@127.0.0.1:3306";
    let pool = get_tidb_pool(&format!("{}/{}", url, "sample")).await;
    let v = element::query_world(7600, pool).await?;
    println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_get_children() -> anyhow::Result<()> {
    let url = "mysql://root:root@127.0.0.1:3306";
    let info_pool = get_tidb_pool(&format!("{}/{}", url, "refno_infos")).await;
    let refno = RefU64(65721589565564);
    let project = element::query_refno_infos(refno, info_pool).await?;
    let pool = get_tidb_pool(&format!("{}/{}", url, project)).await;
    let v = element::query_children(refno, pool).await?;
    println!("v={:?}", v);
    Ok(())
}