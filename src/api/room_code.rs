use aios_core::pdms_types::RefU64;
use arangors_lite::{AqlQuery, Database};
use sqlx::{MySql, Pool, Row};
use crate::aql_api::convert_refno_vec_from_vec_string;

/// 查询房间号
pub async fn query_room_code(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Option<String>> {
    let sql = gen_query_room_code_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    return match result {
        Ok(val) => {
            Ok(Some(val.get::<String, _>("ROOM_NAME")))
        }
        Err(e) => {
            Ok(None)
        }
    };
}

fn gen_query_room_code_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ROOM_NAME FROM ROOM_CODE WHERE REFNO = {}", refno.0));
    sql
}