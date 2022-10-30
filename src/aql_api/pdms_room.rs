use std::collections::HashMap;
use std::env;
use aios_core::pdms_types::RefU64;
use parry3d::bounding_volume::AABB;
use serde::{Serialize, Deserialize};
use sqlx::{MySql, Pool, Row};
use crate::consts::PDMS_ELEMENTS_TABLE;
use crate::data_interface::tidb_manager::AiosDBManager;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RoomInfo {
    pub refno: RefU64,
    pub name: String,
    pub aabb: Option<AABB>,
    pub target_refnos: Vec<RefU64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RoomElementAql {
    pub _key: String,
    pub refno: RefU64,
    pub name: String,
    pub aabb: Option<AABB>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RoomEdgeAql {
    pub _key: String,
    pub _from: String,
    pub _to: String,
}

// pub fn save_room_info_to_arangodb(room_info:HashMap<RefU64>)

/// 获取所有需要计算的房间号
pub async fn query_all_need_compute_room_refno(dbno: Vec<i32>, room_type: &str, pool: &Pool<MySql>) -> anyhow::Result<Vec<(RefU64,String)>> {
    let mut refnos = vec![];
    let sql = gen_query_all_need_compute_room_refno_sql(dbno, room_type);
    let results = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for result in results {
        refnos.push((RefU64(result.get::<i64, _>("ID") as u64),result.get::<String,_>("NAME")));
    }
    Ok(refnos)
}

fn gen_query_all_need_compute_room_refno_sql(dbnos: Vec<i32>, room_type: &str) -> String {
    let mut sql = String::new();
    let mut dbno_str = String::new();
    for dbno in &dbnos {
        dbno_str.push_str(&format!("{} ,", dbno.to_string()));
    }

    sql.push_str(&format!("SELECT ID,NAME FROM {PDMS_ELEMENTS_TABLE} WHERE TYPE = '{}'", room_type));
    if !dbnos.is_empty() {
        dbno_str.remove(dbno_str.len() - 1);
        sql.push_str(&format!("AND NUMBDB IN ({})", dbno_str))
    }
    sql
}


#[tokio::test]
async fn test_query_all_need_compute_room_refno() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let results = query_all_need_compute_room_refno(vec![7200], "ROOM", &pool).await?;
    dbg!(&results);
    Ok(())
}