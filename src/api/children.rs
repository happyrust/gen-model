use std::collections::VecDeque;
use std::env;
use std::fmt::format;
use aios_core::pdms_types::{RefI32Tuple, RefU64, RefU64Vec};
use sqlx::{MySql, Pool, Row};
use crate::consts::PDMS_ELEMENTS_TABLE;
use crate::api::element::query_children;
use crate::data_interface::tidb_manager::AiosDBManager;

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

/// 遍历该节点的所有子节点为指定type的所有数据
pub async fn travel_children_with_type(refno: RefU64, att_type: String, pool: &Pool<MySql>) -> anyhow::Result<Vec<String>> {
    let mut result = vec![];
    let children = travel_children_eles(refno, pool).await?;
    let sql = gen_query_names_from_refnos_sql(children,att_type);
    let vals = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for val in vals {
        let name = val.get::<String, _>("NAME");
        result.push(name);
    }
    Ok(result)
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

fn gen_query_names_from_refnos_sql(refnos: Vec<RefU64>,att_type:String) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT NAME FROM {PDMS_ELEMENTS_TABLE} WHERE TYPE = '{}' AND ID IN ( ",att_type));
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