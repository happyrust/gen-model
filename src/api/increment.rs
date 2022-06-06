use aios_core::pdms_types::{AttrMap, RefU64};
use sqlx::{MySql, Pool, Row};
use aios_core::pdms_data::IncrementData;
use crate::consts::INCREMENT_DATA;

pub async fn query_latest_data(pool:&Pool<MySql>) -> anyhow::Result<Vec<IncrementData>> {
    let mut result = vec![];
    let sql = gen_query_latest_data_sql();
    let vals = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for val in vals {
        let refno = RefU64(val.get::<i64, _>("REFNO") as u64);
        let operate = val.get::<String, _>("OPERATE");
        let version = val.get::<i32, _>("VERSION") as u32;
        let data = val.get::<Vec<u8>,_>("DATA");
        result.push(IncrementData{
            refno,
            attr_data_map: AttrMap::from_bincode_bytes(&data).unwrap(),
            state: operate,
            version,
        });
    }
    Ok(result)
}

fn gen_query_latest_data_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT REFNO,OPERATE,VERSION,DATA FROM {INCREMENT_DATA}"));
    sql
}