use aios_core::pdms_types::{AiosStr, RefU64};
use sqlx::{MySql, Pool, Row};
use crate::query_sql::query_children;
use crate::sql::gen_sql::gen_query_refno_type_sql;
use crate::sql::query_sql;

pub async fn query_refno_type(refno:RefU64, pool:Pool<MySql>) -> anyhow::Result<String> {
    let sql = gen_query_refno_type_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    Ok(result.get::<String,_>(0))
}

pub async fn query_children_pdms_tree(refno:RefU64, pool:Pool<MySql>) -> anyhow::Result<Vec<(RefU64, AiosStr)>> {
    let type_name = query_refno_type(refno,pool.clone()).await?;
    return if type_name == "WORL" {
        query_sql::query_world_children(pool.clone()).await
    } else {
        query_children(refno, pool.clone()).await
    }
}
