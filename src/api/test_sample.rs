use std::env;
use std::time::Instant;
use aios_core::pdms_types::{RefI32Tuple, RefU64};
use sqlx::{MySql, Pool};
use crate::api::attr;
use crate::api::element::*;
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
    use std::sync::Arc;
    use crate::api::element;
    use super::*;

    #[tokio::test]
    async fn test_get_mdb_type() -> anyhow::Result<()> {
        let info_pool = get_test_info_pool().await;
        let pool = get_test_sample_pool().await;
        let project = query_mdb_module_worlds(&pool, &info_pool).await?;
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
        let v = query_world("SAMPLE", "DESI", &pool).await?;
        println!("v={:?}", v);
        Ok(())
    }

    #[tokio::test]
    async fn test_query_world_children() -> anyhow::Result<()> {
        let info_pool = get_test_info_pool().await;
        let pool = get_test_sample_pool().await;
        let v = query_world_children("SAMPLE", "DESI", &pool).await?;
        println!("v={:?}", v);
        Ok(())
    }

    #[tokio::test]
    async fn test_query_children_pdms_tree() -> anyhow::Result<()> {
        let info_pool = get_test_info_pool().await;
        let pool = get_test_sample_pool().await;
        let refno: RefU64 = RefI32Tuple((15392, 0)).into();
        let v = query_children_pdms_tree("SAMPLE", "DESI", refno, &pool).await?;
        println!("v={:?}", v);
        Ok(())
    }

    #[tokio::test]
    async fn test_query_owner_from_id() -> anyhow::Result<()> {
        let info_pool = get_test_info_pool().await;
        let pool = get_test_sample_pool().await;
        let refno: RefU64 = RefI32Tuple((0, 0)).into();
        let v = query_owner_from_id(refno, &pool).await?;
        println!("v={:?}", v);
        Ok(())
    }

    #[tokio::test]
    async fn test_query_implicit_attr() -> anyhow::Result<()> {
        let refno = RefU64::from_two_nums(23548, 402);
        let mgr = AiosDBManager::init_form_config().await?;
        let project = mgr.get_project_name(refno).unwrap();
        dbg!(&project);
        let v = attr::query_implicit_attr(refno, &mgr.get_project_pool(refno).unwrap()).await.unwrap();
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
        let refno = RefU64(105548821299733);
        let mgr = AiosDBManager::init_form_config().await?;
        let project = mgr.get_project_name(refno).unwrap();
        dbg!(&project);
        let v = attr::query_explicit_attr(refno, &mgr.get_project_pool(refno).unwrap()).await?;
        println!("v={:?}", v.to_string_hashmap());
        Ok(())
    }

    #[tokio::test]
    async fn test_query_full_attr() -> anyhow::Result<()> {

        let t = Instant::now();
        let refno = RefU64::from_two_nums(23548, 402);
        let mgr = Arc::new(AiosDBManager::init_form_config().await?);
        let mut handles = vec![];
        for _  in 0..10000 {
            let mgr = mgr.clone();
            let handle = tokio::spawn(async move{
                let project = mgr.get_project_name(refno).unwrap();
                let v = attr::query_full_attr(refno, &mgr.get_project_pool(refno).unwrap()).await.unwrap_or_default();
                // println!("v={:?}", v.to_string_hashmap());
            });
            handles.push(handle);
        }
        futures::future::join_all(handles).await;
        dbg!(t.elapsed().as_millis());
        Ok(())
    }


    #[tokio::test]
    async fn test_get_children() -> anyhow::Result<()> {
        // let info_pool = get_test_info_pool().await;
        // let refno = RefU64(65721589565564);
        // let project = element::query_project_name(refno, &info_pool).await?;
        // let pool = get_test_sample_pool().await;
        // let v = element::query_children(refno, &pool).await?;
        // println!("v={:?}", v);
        Ok(())
    }
}
