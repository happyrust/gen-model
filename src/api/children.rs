use std::collections::VecDeque;
use std::env;
use aios_core::pdms_types::{RefI32Tuple, RefU64, RefU64Vec};
use sqlx::{MySql, Pool};
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

#[tokio::test]
async fn test_travel_children_eles() -> anyhow::Result<()>{
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let refno:RefU64 = RefI32Tuple((23584,5693)).into();
    let v = travel_children_eles(refno,&pool).await?;
    dbg!(&v);
    Ok(())
}