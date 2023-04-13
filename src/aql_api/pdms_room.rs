use std::collections::{HashMap, HashSet};
use std::env;
use aios_core::options::DbOption;
use aios_core::pdms_types::{NounHash, PdmsElement, RefU64, UdaMajorType};
use aios_core::pdms_types::UdaMajorType::T;
use arangors_lite::{AqlQuery, ClientError, Database};
use parry3d::bounding_volume::Aabb;
use regex::Regex;
use serde::{Serialize, Deserialize};
use sqlx::{MySql, Pool, Row};
use crate::api::attr::{get_site_major_from_uda, query_explicit_attr};
use crate::api::children::{get_ancestor_refno_of_type_data, query_ancestor_of_type};
use crate::aql_api::children::query_ancestor_name_of_type_aql;
use crate::aql_api::{convert_refno_vec_from_vec_string, PdmsElementAql};
use crate::consts::PDMS_ELEMENTS_TABLE;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::{get_arangodb_conn_from_db_option, save_arangodb_with_database};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RoomData {
    pub refno: RefU64,
    pub name: String,
    pub aabb: Option<Aabb>,
    pub target_refnos: Vec<RefU64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RoomElementAql {
    pub _key: String,
    pub refno: RefU64,
    pub aabb: Aabb,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RoomEdgeAql {
    pub _key: String,
    pub _from: String,
    pub _to: String,
    pub major: UdaMajorType,
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
pub async fn save_room_info_to_arangodb(aios_mgr: &AiosDBManager, room_infos: HashMap<RefU64, (Aabb, Vec<RefU64>)>, db_option: &DbOption, mut major_map: &mut HashMap<RefU64, UdaMajorType>) -> anyhow::Result<()> {
    let mut room_eles_json = vec![];
    let mut room_edges_json = vec![];
    for (refno, (aabb, target_refnos)) in room_infos {
        room_eles_json.push(RoomElementAql {
            _key: refno.to_url_refno(),
            refno,
            aabb,
        });
        for target_refno in target_refnos {
            // 获取 target_refno 属于哪个专业
            let site = get_ancestor_refno_of_type_data(&aios_mgr, target_refno, "SITE".to_string());
            let mut major = UdaMajorType::NULL;
            if let Some(r) = major_map.get(&site) {
                major = r.clone();
            } else {
                if let Some((_, pool)) = aios_mgr.get_project_pool_by_refno(site).await {
                    if let Some(major_uda) = get_site_major_from_uda(site, &pool).await {
                        major_map.insert(site, major_uda.clone());
                        major = major_uda;
                    }
                }
            }
            let hash = refno.hash_with_another_refno(target_refno);
            room_edges_json.push(RoomEdgeAql {
                _key: hash.to_string(),
                _from: format!("room_eles/{}", refno.to_url_refno()),
                _to: format!("pdms_eles/{}", target_refno.to_url_refno()),
                major,
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

/// 获取该参考号属于哪个房间 room_name_type : 存放房间名的类型
pub async fn query_room_info_from_refno(refno: RefU64, room_name_type: &str, database: &Database) -> anyhow::Result<Option<String>> {
    let refno = format!("pdms_eles/{}", refno.to_url_refno());
    let aql = AqlQuery::new("
    let refno = (for v,e in 1 inbound @id room_edges
                return v._key )[0]
    return refno").bind_var("id", refno);
    let result = database.aql_query::<String>(aql).await;
    return match result {
        Ok(r) => {
            if !r.is_empty() {
                let room_refno = RefU64::from_url_refno(&r[0]);
                if room_refno.is_none() { return Ok(None); }
                let room_refno = room_refno.unwrap();
                let room_name = query_ancestor_name_of_type_aql(database, room_refno, room_name_type).await?;
                if room_name.is_none() { return Ok(None); }
                let room_name = room_name.unwrap();
                Ok(Some(room_name))
            } else {
                Ok(None)
            }
        }
        Err(_) => {
            Ok(None)
        }
    };
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

/// 查找房间下的所有元件的参考号
pub async fn query_room_refnos_aql(refno: RefU64, filter_major: Option<UdaMajorType>, database: &Database) -> anyhow::Result<Vec<RefU64>> {
    let key = format!("pdms_eles/{}", refno.to_url_refno());
    let aql = if filter_major.is_none() {
        AqlQuery::new("
        for e in 0..10 inbound @key pdms_edges
            // filter e.noun == 'PANE'
            for v in 1 outbound CONCAT('room_eles/',e._key) room_edges
                filter v != null
                return v._key
        ").bind_var("key", key)
    } else {
        let filter_data = filter_major.unwrap().to_major_str();
        AqlQuery::new("
        for v,e in 1 outbound @key room_edges
            filter v != null
            filter filter_major == e.major
            return v._key
        ").bind_var("key", key).bind_var("filter_major", filter_data)
    };
    let result: Vec<String> = database.aql_query(aql).await?;
    Ok(convert_refno_vec_from_vec_string(result))
}

/// 查找房间下的所有元件的 pdms_element
pub async fn query_room_pdms_elements_aql(refno: RefU64, filter_major: Option<UdaMajorType>, database: &Database) -> anyhow::Result<Vec<PdmsElement>> {
    let mut r = Vec::new();
    let key = format!("pdms_eles/{}", refno.to_url_refno());
    let aql = if filter_major.is_none() {
        AqlQuery::new("
        for e in 0..10 inbound @key pdms_edges
            filter e.noun == 'PANE'
            for v in 1 outbound CONCAT('room_eles/',e._key) room_edges
                filter v != null
                return { refno:v._key , owner:v.owner , name:v.name,noun:v.noun,version:0,children_count:0 }
        ").bind_var("key", key)
    } else {
        let filter_data = filter_major.unwrap().to_major_str();
        AqlQuery::new("
        for p in 0..10 inbound @key pdms_edges
            filter p.noun == 'PANE'
            for v,e in 1 outbound CONCAT('room_eles/',p._key) room_edges
                filter v != null
                filter @filter_major == e.major
                return { refno:v._key , owner:v.owner , name:v.name,noun:v.noun,version:0,children_count:0 }
        ").bind_var("key", key).bind_var("filter_major", filter_data)
    };
    let results: Vec<PdmsElementAql> = database.aql_query(aql).await.unwrap();
    for result in results {
        if let Some(pdms_element) = result.change_to_pdms_element() {
            r.push(pdms_element);
        }
    }
    Ok(r)
}

/// 通过命名规则获取房间名
pub fn get_room_name_split(name: &str) -> Option<RoomInfo> {
    let room_split = name.split('-').collect::<Vec<_>>();
    if room_split.len() < 3 { return None; }
    let factory = room_split[0].replace("/", "");
    let leave = room_split[1].replace("RM", "").parse().unwrap_or(0);
    let room_name = room_split[2].to_string();
    Some(RoomInfo {
        factory,
        leave,
        room_name,
    })
}

pub async fn query_refno_belong_rooms(refno: RefU64, database: &Database) -> anyhow::Result<Vec<PdmsElement>> {
    let mut set = HashSet::new();
    let mut r = Vec::new();
    let id = format!("pdms_eles/{}", refno.to_url_refno());
    let aql = AqlQuery::new("
    let elements = ( for v in 0..100 inbound @id pdms_edges
                    filter v!= null
                    return v._id )
    let room_refnos = (for element in elements
                        for v in 1 inbound element room_edges
                        filter v!= null
                        return v._key )
    for room_refno in room_refnos
        let id = document('pdms_eles',room_refno)._id
        for v in 0..10 outbound id pdms_edges
            filter v!= null
            filter v.noun == 'FRMW'
            return { refno:v._key , owner:0 , name:v.name,noun:v.noun,version:0,children_count:1 }")
        .bind_var("id", id);
    let results: Vec<PdmsElement> = database.aql_query(aql).await?;
    for result in results {
        let refno = RefU64::from_url_refno(&result.refno);
        if refno.is_none() { continue; }
        let refno = refno.unwrap();
        if set.contains(&refno) { continue; }
        set.insert(refno);
        r.push(PdmsElement {
            refno: refno.to_refno_string(),
            owner: result.owner,
            name: result.name,
            noun: result.noun,
            version: 0,
            children_count: 1,
        })
    }
    Ok(r)
}

#[tokio::test]
async fn test_query_room_info_from_refno() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option(&db_option).await?;
    let refno = RefU64::from_url_refno("24381_178638").unwrap();
    let name = query_room_info_from_refno(refno, "FRMW", &database).await?.unwrap();
    let room_name = get_room_name_split(&name).unwrap();
    dbg!(&room_name);
    Ok(())
}

#[tokio::test]
async fn test_query_refno_belong_rooms() -> anyhow::Result<()> {
    use config::{Config, ConfigError, Environment, File};
    let s = Config::builder()
        .add_source(File::with_name("DbOption"))
        .build()?;
    let db_option: DbOption = s.try_deserialize().unwrap();
    let database = get_arangodb_conn_from_db_option(&db_option).await?;
    let refno = RefU64::from_url_refno("24383_68084").unwrap();
    let name = query_refno_belong_rooms(refno, &database).await?;
    dbg!(&name);
    Ok(())
}

#[test]
fn test_json() {
    let str = vec![T];
    let json = serde_json::to_string(&str).unwrap();
    dbg!(&json);
}

#[test]
fn test_match_room_name() {
    let re = Regex::new(r"^/\d+[A-Z]{2}-RM\d{2}-R\d{3}$").unwrap();

    dbg!(re.is_match("/123AB-RM03-R310"));
    dbg!(re.is_match("/456CD-RM03-R312"));
    dbg!(re.is_match("/789EF-RM11-R976"));
    dbg!(!re.is_match("/1RA-RM03-R312"));
    dbg!(!re.is_match("/1NX-RM11-R976"));
    dbg!(!re.is_match("/12A-RM11-R976"));
}