use std::env;
use sqlx::{MySql, Pool, Row};
use crate::consts::METADATA_TABLE;
use aios_core::metadata_manager::{MetadataManagerTableData, MetadataManagerTreeNode, ShowMetadataManagerTableData};
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
            english_define: "".to_string(),
            chinese_define: "".to_string(),
            classify_code: "".to_string(),
            classify_name: "".to_string(),
            custom_item: "".to_string(),
            desc: "".to_string(),
            state: false,
            owned_name: "".to_string(),
        }));
    }
    Ok(None)
}

pub async fn query_metadata_tree_children(id: u64, pool: &Pool<MySql>) -> anyhow::Result<Vec<MetadataManagerTreeNode>> {
    let mut children = vec![];
    let sql = gen_query_metadata_tree_children_sql(id);
    let owner = id;
    let result = sqlx::query(&sql).fetch_all(pool).await;
    if let Ok(results) = result {
        for result in results {
            let id = result.get::<u64, _>("ID");
            let chinese_name = result.get::<String, _>("CHINESE_NAME");
            children.push(MetadataManagerTreeNode {
                id,
                owner,
                user_code: "".to_string(),
                chinese_name,
                english_name: "".to_string(),
                english_define: "".to_string(),
                chinese_define: "".to_string(),
                classify_code: "".to_string(),
                classify_name: "".to_string(),
                custom_item: "".to_string(),
                desc: "".to_string(),
                state: false,
                owned_name: "".to_string(),
            })
        }
    }
    Ok(children)
}

pub async fn query_metadata_tree_node_data(id: u64, pool: &Pool<MySql>) -> anyhow::Result<MetadataManagerTreeNode> {
    let sql = gen_query_metadata_tree_node_data(id);
    let result = sqlx::query(&sql).fetch_one(pool).await;
    if let Ok(result) = result {
        let user_code = result.get::<String, _>("USER_CODE");
        let english_name = result.get::<String, _>("ENGLISH_NAME");
        let chinese_name = result.get::<String, _>("CHINESE_NAME");
        let english_define = result.get::<String, _>("ENGLISH_DEFINE");
        let chinese_define = result.get::<String, _>("CHINESE_DEFINE");
        let classify_code = result.get::<String, _>("CLASSIFY_CODE");
        let classify_name = result.get::<String, _>("CLASSIFY_NAME");
        let custom_item = result.get::<String, _>("CUSTOM_ITEM");
        let description = result.get::<String, _>("DESCRIPTION");
        let state = result.get::<bool, _>("STATE");
        let owned_name = result.get::<String, _>("OWNED_NAME");
        return Ok(MetadataManagerTreeNode {
            id,
            owner: 0,
            user_code,
            chinese_name,
            english_name,
            english_define,
            chinese_define,
            classify_code,
            classify_name,
            custom_item,
            desc: description,
            state,
            owned_name,
        });
    }
    Ok(MetadataManagerTreeNode::default())
}

pub async fn query_metadata_table_sql(id: u64, pool: &Pool<MySql>) -> anyhow::Result<Vec<MetadataManagerTableData>> {
    let mut datas = Vec::new();
    let sql = gen_query_metadata_table_data_sql(id);
    let result = sqlx::query(&sql).fetch_all(pool).await;
    if let Ok(results) = result {
        for result in results {
            let code = result.get::<String, _>("CODE");
            let data_type = result.get::<String, _>("DATA_TYPE");
            let data_constraint = result.get::<String, _>("DATA_CONSTRAINT");
            let b_multi = result.get::<bool, _>("B_MULTI");
            let english_name = result.get::<String, _>("ENGLISH_NAME");
            let chinese_name = result.get::<String, _>("CHINESE_NAME");
            let english_define = result.get::<String, _>("ENGLISH_DEFINE");
            let chinese_define = result.get::<String, _>("CHINESE_DEFINE");
            let unit = result.get::<String, _>("UNIT");
            let group = result.get::<String, _>("GROUPINGS");
            let custom_item = result.get::<String, _>("CUSTOM_ITEM");
            let desc = result.get::<String, _>("DESCRIPTION");
            let owned_name = result.get::<String, _>("OWNED_NAME");
            let state = result.get::<bool, _>("STATE");
            let group = if group.is_empty() { "[0]".to_string() } else { group };

            datas.push(MetadataManagerTableData {
                id,
                code,
                data_type,
                data_constraint,
                b_multi,
                english_name,
                chinese_name,
                english_define,
                chinese_define,
                unit,
                group,
                custom_item,
                desc,
                state,
                owned_name,
            });
        }
    }
    Ok(datas)
}

pub async fn query_metadata_table_code_sql(id: u64, pool: &Pool<MySql>) -> anyhow::Result<Vec<String>> {
    let mut datas = Vec::new();
    let sql = gen_query_metadata_table_code_data_sql(id);
    let result = sqlx::query(&sql).fetch_all(pool).await;
    if let Ok(results) = result {
        for result in results {
            let code = result.get::<String, _>("CODE");
            datas.push(code);
        }
    }
    Ok(datas)
}

pub async fn query_tree_node_detail(id: u64, pool: Pool<MySql>) -> anyhow::Result<Option<MetadataManagerTreeNode>> {
    let sql = gen_query_metadata_tree_data_sql(id);
    let result = sqlx::query(&sql).fetch_one(&pool).await;
    if let Ok(result) = result {
        let id = result.get::<u64, _>("ID");
        let code = result.get::<String, _>("USER_CODE");
        let english_name = result.get::<String, _>("ENGLISH_NAME");
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
    sql.push_str(&format!("SELECT ID,CHINESE_NAME FROM {METADATA_TABLE} WHERE OWNER = {}", id));
    sql
}

/// USER_CODE,ENGLISH_NAME,CHINESE_NAME , ENGLISH_DEFINE,CHINESE_DEFINE ,CLASSIFY_CODE,CLASSIFY_NAME, CUSTOM_ITEM ,DESCRIPTION,STATE,OWNED_NAME
fn gen_query_metadata_tree_node_data(id: u64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT USER_CODE,ENGLISH_NAME,CHINESE_NAME , ENGLISH_DEFINE,CHINESE_DEFINE ,CLASSIFY_CODE,CLASSIFY_NAME, CUSTOM_ITEM ,DESCRIPTION,STATE,OWNED_NAME FROM {METADATA_TABLE} WHERE ID = {}", id));
    sql
}

fn gen_query_metadata_table_data_sql(id: u64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT CODE,DATA_TYPE,DATA_CONSTRAINT,B_MULTI,ENGLISH_NAME,CHINESE_NAME , ENGLISH_DEFINE,CHINESE_DEFINE , UNIT , GROUPINGS,CUSTOM_ITEM ,DESCRIPTION,STATE,OWNED_NAME FROM {METADATA_DATA} WHERE ID = {}", id));
    sql
}

fn gen_query_metadata_table_code_data_sql(id: u64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT CODE FROM {METADATA_DATA} WHERE ID = {}", id));
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
    let id = 5238326229088613117;
    // let data = query_metadata_tree_children(id, &pool).await?;
    let data = query_metadata_tree_node_data(id,&pool).await?;
    dbg!(&data);
    Ok(())
}

#[tokio::test]
async fn test_query_metadata_table_sql() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let id = 5238326229088613117;
    let data = query_metadata_table_sql(id, &pool).await?;
    dbg!(&data);
    Ok(())
}