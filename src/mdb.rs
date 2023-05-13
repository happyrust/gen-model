use std::env;
use dashmap::DashMap;
use sqlx::{MySql, Pool};
use crate::api::attr::{query_explicit_attr, query_implicit_attr, query_numbdbs_by_mdb};
use crate::api::element::{query_children, query_types_refnos_names};
use crate::data_interface::tidb_manager::AiosDBManager;

/// 获取设计库所有的 mdb 和 mdb 下面的 db_num
pub async fn get_project_mdb(project_pool: &Pool<MySql>) -> anyhow::Result<DashMap<String, Vec<u32>>> {
    let mut result = DashMap::new();
    // 获取到所有的 mdb
    let mdb = query_types_refnos_names(&vec!["MDB"], project_pool,None).await?;
    for (mdb_refno, mut mdb_name) in mdb {
        if mdb_name.starts_with("/") { mdb_name.remove(0); }
        let mdb_attr = query_explicit_attr(mdb_refno, project_pool).await?;
        let dbs = mdb_attr.get_refu64_vec("CURD");
        if dbs.is_none() { continue; }
        let dbs = dbs.unwrap();
        let numbdbs = query_numbdbs_by_mdb(dbs, project_pool).await?;
        result.entry(mdb_name).or_insert(numbdbs);
    }
    Ok(result)
}

#[tokio::test]
async fn test_get_project_mdb() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let r = get_project_mdb(&pool).await?;
    dbg!(&r);
    Ok(())
}