use std::env;
use sqlx::{MySql, Pool, Row};
use crate::consts::METADATA_TABLE;
use aios_core::metadata_manager::{MetadataManagerTableData, MetadataManagerTreeNode};
use nom::number::streaming::u64;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::consts::METADATA_DATA;

/// 找到元数据管理树结构的根节点
pub async fn query_metadata_tree_root(pool: &Pool<MySql>) -> anyhow::Result<Option<MetadataManagerTreeNode>> {
    let sql = gen_query_metadata_tree_root_sql();
    let result = sqlx::query(&sql).fetch_one(pool).await;
    if let Ok(result) = result {
        let id = result.get::<u64, _>("ID");
        let chinese_name = result.get::<String, _>("CHINESE_NAME");
        return Ok(Some(MetadataManagerTreeNode {
            id,
            owner: 0,
            user_code: "".to_string(),
            chinese_name,
            english_name: "".to_string(),
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
            let owner = result.get::<u64, _>("OWNER");
            let chinese_name = result.get::<String, _>("CHINESE_NAME");
            children.push(MetadataManagerTreeNode {
                id,
                owner,
                user_code: "".to_string(),
                chinese_name,
                english_name: "".to_string(),
            })
        }
    }
    Ok(children)
}

pub async fn query_metadata_table_sql(id: u64, pool: &Pool<MySql>) -> anyhow::Result<Vec<MetadataManagerTableData>> {
    let mut datas = Vec::new();
    let sql = gen_query_metadata_table_data_sql(id);
    let result = sqlx::query(&sql).fetch_all(pool).await;
    if let Ok(results) = result {
        for result in results {
            let code = result.get::<String, _>("CODE");
            let name = result.get::<String, _>("NAME");
            let b_null = result.get::<bool, _>("B_NULL");
            let data_type = result.get::<i8, _>("DATA_TYPE") as u8;
            let unit = result.get::<i8, _>("UNIT") as u8;
            let desc = result.get::<String, _>("DESCRIPTION");
            let scope = result.get::<String, _>("SCOPE");
            datas.push(MetadataManagerTableData {
                id,
                code,
                name,
                b_null,
                data_type,
                unit,
                desc,
                scope,
            });
        }
    }
    Ok(datas)
}

pub async fn query_tree_node_detail(id: u64,pool:Pool<MySql>) -> anyhow::Result<Option<MetadataManagerTreeNode>> {
    let sql = gen_query_metadata_tree_data_sql(id);
    let result = sqlx::query(&sql).fetch_one(&pool).await;
    if let Ok(result) = result {
        let id = result.get::<u64,_>("ID");
        let code = result.get::<String,_>("USER_CODE");
        let english_name = result.get::<String,_>("ENGLISH_NAME");

    }
    Ok(None)
}

fn gen_query_metadata_tree_root_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID,CHINESE_NAME FROM {METADATA_TABLE} WHERE OWNER = 0"));
    sql
}

fn gen_query_metadata_tree_data_sql(id: u64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID,USER_CODE,ENGLISH_NAME FROM {METADATA_TABLE} WHERE ID = {}", id));
    sql
}

fn gen_query_metadata_tree_children_sql(id: u64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID,OWNER,CHINESE_NAME FROM {METADATA_TABLE} WHERE OWNER = {}", id));
    sql
}

fn gen_query_metadata_table_data_sql(id: u64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT CODE,NAME , B_NULL , DATA_TYPE , UNIT , DESCRIPTION ,SCOPE FROM {METADATA_DATA} WHERE ID = {}", id));
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
    let data = query_metadata_tree_children(id, &pool).await?;
    dbg!(&data);
    Ok(())
}

#[tokio::test]
async fn test_query_metadata_table_sql() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let id = 11787254984997374616;
    let data = query_metadata_table_sql(id, &pool).await?;
    dbg!(&data);
    Ok(())
}