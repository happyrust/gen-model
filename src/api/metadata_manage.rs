use std::env;
use sqlx::{MySql, Pool, Row};
use crate::consts::METADATA_TABLE;
use aios_core::metadata_manager::MetadataManagerTreeNode;
use crate::data_interface::tidb_manager::AiosDBManager;

/// 找到元数据管理树结构的根节点
pub async fn query_metadata_tree_root(pool: &Pool<MySql>) -> anyhow::Result<Option<MetadataManagerTreeNode>> {
    let sql = gen_query_metadata_tree_root_sql();
    let result = sqlx::query(&sql).fetch_one(pool).await;
    if let Ok(result) = result {
        let id = result.get::<u64, _>("ID");
        let user_code = result.get::<String, _>("USER_CODE");
        let chinese_name = result.get::<String, _>("CHINESE_NAME");
        let english_name = result.get::<String, _>("ENGLISH_NAME");
        return Ok(Some(MetadataManagerTreeNode {
            id,
            owner: 0,
            user_code,
            chinese_name,
            english_name,
        }));
    }
    Ok(None)
}

pub async fn query_metadata_tree_children(id: u64, pool: &Pool<MySql>) -> anyhow::Result<Vec<MetadataManagerTreeNode>> {
    let mut children = vec![];
    let sql = gen_query_metadata_tree_children_sql(id);
    let result = sqlx::query(&sql).fetch_all(pool).await;
    if let Ok(results) = result {
        for result in results {
            let id = result.get::<u64, _>("ID");
            let owner = result.get::<u64,_>("OWNER");
            let user_code = result.get::<String, _>("USER_CODE");
            let chinese_name = result.get::<String, _>("CHINESE_NAME");
            let english_name = result.get::<String, _>("ENGLISH_NAME");
            children.push(MetadataManagerTreeNode {
                id,
                owner,
                user_code,
                chinese_name,
                english_name,
            })
        }
    }
    Ok(children)
}

fn gen_query_metadata_tree_root_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID,USER_CODE,CHINESE_NAME,ENGLISH_NAME FROM {METADATA_TABLE} WHERE OWNER = 0"));
    sql
}

fn gen_query_metadata_tree_children_sql(id: u64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID,OWNER,USER_CODE,CHINESE_NAME,ENGLISH_NAME FROM {METADATA_TABLE} WHERE OWNER = {}", id));
    sql
}

#[tokio::test]
async fn test_query_metadata_tree_root() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let data = query_metadata_tree_root(&pool).await?.unwrap();
    dbg!(&data);
    Ok(())
}

#[tokio::test]
async fn test_query_metadata_tree_children() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let id = 11787254984997374616;
    let data = query_metadata_tree_children(id,&pool).await?;
    dbg!(&data);
    Ok(())
}