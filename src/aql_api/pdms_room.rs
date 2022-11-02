use std::collections::HashMap;
use std::env;
use aios_core::pdms_types::RefU64;
use parry3d::bounding_volume::AABB;
use serde::{Serialize, Deserialize};
use sqlx::{MySql, Pool, Row};
use crate::consts::PDMS_ELEMENTS_TABLE;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::{get_arangodb_conn_from_db_option, save_arangodb_with_database};
use crate::options::DbOption;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RoomData {
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
    // pub aabb: Option<AABB>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RoomEdgeAql {
    pub _key: String,
    pub _from: String,
    pub _to: String,
}

/// 房间对应的信息
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RoomInfo {
    // 厂房
    pub factory: String,
    // 层位
    pub leave: i32,
    // 房间名
    pub room_name: String,
}

/// 将房间信息保存到图数据库
pub async fn save_room_info_to_arangodb(room_infos: HashMap<RefU64, (String, Vec<RefU64>)>, db_option: &DbOption) -> anyhow::Result<()> {
    let mut room_eles_json = vec![];
    let mut room_edges_json = vec![];
    for (refno, (room_name, target_refnos)) in room_infos {
        room_eles_json.push(RoomElementAql {
            _key: refno.to_url_refno(),
            refno,
            name: room_name,
        });
        for target_refno in target_refnos {
            let hash = refno.hash_with_another_refno(target_refno);
            room_edges_json.push(RoomEdgeAql {
                _key: hash.to_string(),
                _from: format!("room_eles/{}", refno.to_url_refno()),
                _to: format!("pdms_eles/{}", target_refno.to_url_refno()),
            })
        }
    }
    let database = get_arangodb_conn_from_db_option(&db_option).await?;
    let room_eles_json = serde_json::to_value(&room_eles_json);
    if let Ok(room_eles_json) = room_eles_json {
        save_arangodb_with_database(room_eles_json, "room_eles", &database).await?;
    }
    let room_edges_json = serde_json::to_value(&room_edges_json);
    if let Ok(room_edges_json) = room_edges_json {
        save_arangodb_with_database(room_edges_json, "room_edges", &database).await?;
    }
    Ok(())
}

/// 获取所有需要计算的房间号
pub async fn query_all_need_compute_room_refno(dbno: &Vec<i32>, room_type: &str, filter_name: Option<&str>, pool: &Pool<MySql>) -> anyhow::Result<Vec<(RefU64, String)>> {
    let mut refnos = vec![];
    let sql = gen_query_all_need_compute_room_refno_sql(dbno, room_type, filter_name);
    let results = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for result in results {
        refnos.push((RefU64(result.get::<i64, _>("ID") as u64), result.get::<String, _>("NAME")));
    }
    Ok(refnos)
}

fn gen_query_all_need_compute_room_refno_sql(dbnos: &Vec<i32>, room_type: &str, filter_name: Option<&str>) -> String {
    let mut sql = String::new();

    sql.push_str(&format!("SELECT ID,NAME FROM {PDMS_ELEMENTS_TABLE} WHERE TYPE = '{}'", room_type));

    if !dbnos.is_empty() {
        let mut dbno_str = String::new();
        for dbno in dbnos {
            dbno_str.push_str(&format!("{} ,", dbno.to_string()));
        }

        dbno_str.remove(dbno_str.len() - 1);
        sql.push_str(&format!("AND NUMBDB IN ({})", dbno_str))
    }

    if filter_name.is_some() {
        sql.push_str(&format!("AND NAME LIKE '%{}%'", filter_name.unwrap()))
    }
    sql
}

/// 通过命名规则获取房间名
pub fn get_room_name_split(name: &str) -> Option<RoomInfo> {
    let room_split = name.split('-').collect::<Vec<_>>();
    if room_split.len() < 3 { return None; }
    let factory = room_split[0].replace("/","");
    let leave = room_split[1].replace("RM","").parse().unwrap_or(0);
    let room_name = room_split[2].to_string();
    Some(RoomInfo{
        factory,
        leave,
        room_name
    })
}


#[test]
fn test_get_room_name_split() {
    let name = "/1AR-RM01-A109";
    let r = get_room_name_split(name).unwrap();
    dbg!(&r);
}