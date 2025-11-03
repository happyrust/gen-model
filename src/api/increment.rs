use std::collections::{BTreeMap, VecDeque};
use aios_core::AttrMap;
use aios_core::pdms_types::*;
use sqlx::{Error, MySql, Pool, Row};
use aios_core::pdms_data::{NewDataOperate};
use chrono::DateTime;
use sqlx::mysql::MySqlRow;
use crate::consts::INCREMENT_DATA;
use serde::{Serialize, Deserialize};


pub async fn query_latest_data(version: u32, pool: &Pool<MySql>) -> anyhow::Result<Vec<IncrementDataSql>> {
    let mut result = Vec::new();
    let sql = gen_query_latest_data_sql(version);
    let vals = sqlx::query(&sql).fetch_all(pool).await;
    if let Ok(vals) = vals {
        for val in vals {
            let id = val.get::<String, _>("ID");
            let refno = RefU64(val.get::<i64, _>("REFNO") as u64);
            let operate = val.get::<i32, _>("OPERATE");
            let version = val.get::<i32, _>("VERSION") as u32;
            let user = val.get::<String, _>("USER");
            let old_data = AttrMap::from_rkvy_compress_bytes(&val.get::<Vec<u8>, _>("OLD_DATA"))?;
            let new_data = AttrMap::from_rkvy_compress_bytes(&val.get::<Vec<u8>, _>("NEW_DATA"))?;
            let time = val.get::<String, _>("TIME");
            result.push(IncrementDataSql {
                id,
                refno,
                operate: EleOperation::from(operate),
                version,
                user,
                old_data,
                new_data,
                time,
            });
        }
    }
    Ok(result)
}


fn gen_query_latest_data_sql(version: u32) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT * FROM {INCREMENT_DATA} WHERE VERSION > {}", version));
    sql
}