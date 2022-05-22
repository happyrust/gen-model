use std::env;
use std::time::Instant;
use aios_core::pdms_types::{RefI32Tuple, RefU64};
use sqlx::{MySql, Pool};
use crate::api::attr;
use crate::api::element::{query_children_pdms_tree, query_mdb_module_worlds, query_owner_from_id, query_project_name, query_world, query_world_children};
use crate::consts::PDMS_INFO_DB;
use crate::data_interface::tidb_manager::AiosDBManager;

pub async fn get_test_sample_pool() -> Pool<MySql> {
    let _ = dotenv::dotenv();
    let conn_str = env::var("DATABASE_URL").unwrap();
    AiosDBManager::get_db_pool(&conn_str, "sample").await.unwrap()
}

pub async fn get_test_info_pool() -> Pool<MySql> {
    let _ = dotenv::dotenv();
    let conn_str = env::var("DATABASE_URL").unwrap();
    AiosDBManager::get_db_pool(&conn_str, PDMS_INFO_DB).await.unwrap()
}


#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_mdb_type() -> anyhow::Result<()> {
        let info_pool = get_test_info_pool().await;
        let pool = get_test_sample_pool().await;
        let project = query_mdb_module_worlds(pool, info_pool).await?;
        if let Some(v) = project.get("/SAMPLE") {
            if let Some(val) = v.get("DESI") {
                println!("val={:?}", val);
            }
        }
        println!("v={:?}", project);
        Ok(())
    }

    #[tokio::test]
    async fn test_query_world() -> anyhow::Result<()> {
        let info_pool = get_test_info_pool().await;
        let pool = get_test_sample_pool().await;
        let v = query_world("SAMPLE", "DESI", pool.clone()).await?;
        println!("v={:?}", v);
        Ok(())
    }

    #[tokio::test]
    async fn test_query_world_children() -> anyhow::Result<()> {
        let info_pool = get_test_info_pool().await;
        let pool = get_test_sample_pool().await;
        let v = query_world_children("SAMPLE", "DESI", pool.clone()).await?;
        println!("v={:?}", v);
        Ok(())
    }

    #[tokio::test]
    async fn test_query_children_pdms_tree() -> anyhow::Result<()> {
        let info_pool = get_test_info_pool().await;
        let pool = get_test_sample_pool().await;
        let refno: RefU64 = RefI32Tuple((15392, 0)).into();
        let v = query_children_pdms_tree("SAMPLE", "DESI", refno, pool.clone()).await?;
        println!("v={:?}", v);
        Ok(())
    }

    #[tokio::test]
    async fn test_query_owner_from_id() -> anyhow::Result<()> {
        let info_pool = get_test_info_pool().await;
        let pool = get_test_sample_pool().await;
        let refno: RefU64 = RefI32Tuple((0, 0)).into();
        let v = query_owner_from_id(refno, pool.clone()).await?;
        println!("v={:?}", v);
        Ok(())
    }

    #[tokio::test]
    async fn test_query_implicit_attr() -> anyhow::Result<()> {
        let info_pool = get_test_info_pool().await;
        let pool = get_test_sample_pool().await;
        let refno = RefU64(103010495627266);
        let project = query_project_name(refno, info_pool).await?;
        dbg!(&project);
        let v = attr::query_implicit_attr(refno, pool).await.unwrap();
        println!("v={:?}", v.to_string_hashmap());
        Ok(())
    }

    #[tokio::test]
    async fn test_get_world_children() -> anyhow::Result<()> {
        // let url = env::var("DATABASE_URL")?;
        // let info_pool = get_tidb_pool(&format!("{}/{}", url, PDMS_INFO_DB)).await;
        // let refno = RefU64(66108136620032);
        // let project = query_refno_infos(refno, info_pool).await?;
        // let pool = get_tidb_pool(&format!("{}/{}", url, project)).await;
        // let v = element::query_children_pdms_tree("SAMPLE","DESI",refno, pool).await?;
        // println!("v={:?}", v);
        Ok(())
    }

    #[tokio::test]
    async fn test_query_explicit_attr() -> anyhow::Result<()> {
        let info_pool = get_test_info_pool().await;
        let pool = get_test_sample_pool().await;
        let refno = RefU64(105548821299733);
        let project = query_project_name(refno, info_pool).await?;
        let v = attr::query_explicit_attr(refno, pool).await?;
        println!("v={:?}", v.to_string_hashmap());
        Ok(())
    }

    #[tokio::test]
    async fn test_query_full_attr() -> anyhow::Result<()> {
        let info_pool = get_test_info_pool().await;
        let pool = get_test_sample_pool().await;
        let refno = RefU64::from_two_nums(23548, 402);
        let project = query_project_name(refno, info_pool).await?;
        let t = Instant::now();
        let v = attr::query_full_attr(refno, pool).await?;
        dbg!(t.elapsed().as_millis());
        println!("v={:?}", v.to_string_hashmap());
        Ok(())
    }
}
