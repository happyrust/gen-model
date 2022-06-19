use std::collections::VecDeque;
use std::env;
use std::fmt::format;
use aios_core::pdms_types::{EleTreeNode, RefI32Tuple, RefU64, RefU64Vec};
use calamine::Error::De;
use sqlx::{MySql, Pool, Row};
use crate::consts::PDMS_ELEMENTS_TABLE;
use crate::api::element::{query_children, query_owner_from_id, query_refno_type};
use crate::data_interface::tidb_manager::AiosDBManager;
use serde::{Serialize, Deserialize};

/// 遍历该节点下的 children (包含自己)
pub async fn travel_children_eles(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Vec<RefU64>> {
    let mut result = vec![];
    let mut deque = VecDeque::new();
    deque.push_back(refno);
    result.push(refno);
    while deque.len() > 0 {
        let refno = deque.pop_front().unwrap();
        let children = query_children(refno, pool).await?;
        for (refno, _) in children {
            deque.push_back(refno);
            result.push(refno);
        }
    }
    Ok(result)
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SimpleNodeDataForPlat {
    pub refno: String,
    pub name: String,
    pub owner: String,
}

/// 遍历该节点的所有子节点为指定type的所有数据 返回 refno name owner
pub async fn travel_children_with_type(refno: RefU64, att_type: String, pool: &Pool<MySql>) -> anyhow::Result<Vec<EleTreeNode>> {
    let mut result = vec![];
    let children = travel_children_eles(refno, pool).await?;
    let sql = gen_query_names_from_refnos_with_type_sql(children, att_type);
    let vals = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for val in vals {
        let refno = RefU64(val.get::<i64, _>("ID") as u64);
        let name = val.get::<String, _>("NAME");
        let owner = RefU64(val.get::<i64, _>("OWNER") as u64);
        result.push(EleTreeNode {
            refno,
            name,
            owner,
            ..Default::default()
        });
    }
    Ok(result)
}

/// 遍历该节点的所有子节点为指定refno的所有数据 返回 refno name owner
pub async fn travel_children_with_refno(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Vec<RefU64>> {
    let children = travel_children_eles(refno, pool).await?;
    Ok(children)
}

/// 查询指定type的children 的 id 、 name
pub async fn query_children_id_name_with_type(refno: RefU64, att_type: &str, pool: &Pool<MySql>) -> anyhow::Result<Vec<(RefU64, String)>> {
    let mut result = vec![];
    let sql = gen_query_children_id_name_with_type_sql(refno, att_type);
    let vals = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for val in vals {
        let child_refno = RefU64(val.get::<i64, _>("ID") as u64);
        let name = val.get::<String, _>("NAME");
        result.push((child_refno, name));
    }
    Ok(result)
}

/// 模糊查询 类型为 att_type ，name 中包含指定值的所有 refno和 name
pub async fn fuzzy_query_refnos_by_name(att_type: String, name: String, pool: &Pool<MySql>) -> anyhow::Result<Vec<(RefU64, String)>> {
    let mut result = vec![];
    let sql = gen_fuzzy_query_refnos_by_name_sql(att_type, name);
    let vals = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for val in vals {
        let refno = RefU64(val.get::<i64, _>("ID") as u64);
        let name = val.get::<String, _>("NAME");
        result.push((refno, name));
    }
    Ok(result)
}

/// 通过 refno 获取 owner 和 owner 的type
pub async fn query_owner_type_from_id(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Option<(RefU64, String)>> {
    if let Ok(Some(owner)) = query_owner_from_id(refno, pool).await {
        if let Ok(att_type) = query_refno_type(owner, pool).await {
            return Ok(Some((owner, att_type)));
        }
    }
    Ok(None)
}

fn gen_query_names_from_refnos_with_type_sql(refnos: Vec<RefU64>, att_type: String) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID,NAME,OWNER FROM {PDMS_ELEMENTS_TABLE} WHERE TYPE = '{}' AND ID IN ( ", att_type));
    let mut insert_sql = String::new();
    for refno in refnos {
        insert_sql.push_str(&format!("{},", refno.0));
    }
    insert_sql.remove(insert_sql.len() - 1);
    sql.push_str(&format!("{} );", insert_sql));
    sql
}

fn gen_query_names_from_refnos_sql(refnos: Vec<RefU64>) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID,NAME,OWNER FROM {PDMS_ELEMENTS_TABLE} WHERE ID IN ( "));
    let mut insert_sql = String::new();
    for refno in refnos {
        insert_sql.push_str(&format!("{},", refno.0));
    }
    insert_sql.remove(insert_sql.len() - 1);
    sql.push_str(&format!("{} );", insert_sql));
    sql
}

fn gen_query_names_from_refnos_with_name_sql(refnos: Vec<RefU64>, name: String) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID,NAME,OWNER FROM {PDMS_ELEMENTS_TABLE} WHERE NAME = '{}' IN ( ", name));
    let mut insert_sql = String::new();
    for refno in refnos {
        insert_sql.push_str(&format!("{},", refno.0));
    }
    insert_sql.remove(insert_sql.len() - 1);
    sql.push_str(&format!("{} );", insert_sql));
    sql
}


fn gen_query_children_id_name_with_type_sql(refno: RefU64, att_type: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID,NAME FROM {PDMS_ELEMENTS_TABLE} WHERE OWNER = {} AND TYPE = '{}' ", refno.0, att_type));
    sql
}

fn gen_fuzzy_query_refnos_by_name_sql(att_type: String, name: String) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID,NAME FROM {PDMS_ELEMENTS_TABLE} WHERE TYPE = '{}' AND NAME LIKE '%{}%'", att_type, name));
    sql
}

fn gen_query_owner_type_from_id(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT OWNER,TYPE FROM {PDMS_ELEMENTS_TABLE} WHERE ID = {} AND IS_DEL = 0 ", refno.0));
    sql
}


#[tokio::test]
async fn test_travel_children_eles() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let refno: RefU64 = RefI32Tuple((23584, 5693)).into();
    let v = travel_children_eles(refno, &pool).await?;
    dbg!(&v);
    Ok(())
}