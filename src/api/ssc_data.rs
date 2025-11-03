use std::collections::VecDeque;
use std::sync::Arc;
use aios_core::pdms_types::*;
use dashmap::{DashMap, DashSet};
use lazy_static::lazy_static;
use sqlx::{Error, Executor, MySql, Pool, Row};
use crate::consts::ROOM_CODE;
use serde::{Serialize, Deserialize};
use sqlx::mysql::MySqlRow;
use crate::consts::PDMS_SSC_ELEMENTS_TABLE;
use std::collections::HashMap;
use std::env;
use std::fmt::format;
use std::fs::File;
use std::io::Read;
use aios_core::accel_tree::acceleration_tree::AccelerationTree;
use aios_core::options::DbOption;

use parry3d::math::Point;
use crate::api::children::{travel_children_with_refno, travel_children_with_type};
use crate::api::element::{query_ele_node, query_elenode_without_children_count, query_elenodes_without_children_count};
use crate::aql_api::{change_vec_refnos_into_vec_string, convert_refno_vec_from_vec_string, PdmsRefnoNameAql};
use crate::aql_api::children::query_travel_children_aql;
use crate::aql_api::pdms_room::{get_room_name_split, query_all_need_compute_room_refno};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::arangodb::ArDatabase;
use crate::ssc::parse_room_info_from_excel;

// 缓存该参考号的 owner 和 owner 的 type
lazy_static! {
    pub static ref SSC_OWNER_MAP: DashMap<RefU64,(RefU64,String)> = {
        let mut s = DashMap::new();
        s
    };
}

#[derive(Debug, Default, Clone, Eq, PartialEq, Serialize, Deserialize, Hash)]
pub struct SscEleNode {
    pub refno: RefU64,
    pub noun: String,
    pub name: String,
    pub owner: RefU64,
    pub room_code: String,
}

/// 通过指定包围盒计算包围盒中的房间的所有节点
pub async fn get_room_refnos_from_spa_tree_aql(room_refno: RefU64, database: &ArDatabase) -> anyhow::Result<Vec<RefU64>> {
    let mut room_map = vec![];
    let refno = format!("room_eles/{}", room_refno.to_string());
    let aql = AqlQuery::new("\
    for v in 1 outbound @id room_edges
        filter v != null
        return {
            'refno':v._key,
        }").bind_var("id", refno)
        .bind_var("id", room_refno.to_string())
        ;
    let results: Vec<String> = database.aql_query(aql).await?;
    let results = convert_refno_vec_from_vec_string(results);
    // let mut b_insert_self = true; // 需要将自己也加到 target_refnos 里面 方便显示 pane
    for result in results {
        room_map.push(result);
        // if b_insert_self {
        //     let children = query_travel_children_aql(database, room_refno).await?;
        //     for child in children {
        //         let refno = child.refno;
        //         if refno.is_err() { continue; }
        //         let refno = refno.unwrap();
        //         room_map.entry(name.last().unwrap_or(&"").to_string()).or_insert_with(Vec::new).push(refno);
        //     }
        //
        //     b_insert_self = false;
        // }
    }
    Ok(room_map)
}

/// 获取所有带有房间号的节点属性
pub async fn query_all_room_data(pool: &Pool<MySql>) -> anyhow::Result<HashMap<RefU64, SscEleNode>> {
    let sql = gen_query_all_room_data_sql();
    let vals = sqlx::query(&sql).fetch_all(pool).await?;
    let mut refno_room_map = DashMap::new();
    let mut sqls = vec![];
    for val in vals {
        let refno = RefU64(val.get::<i64, _>("REFNO") as u64);
        let room_name = val.get::<String, _>("ROOM_NAME");
        refno_room_map.insert(refno, room_name);
        sqls.push(refno);
    }
    if let Ok(elenodes) = query_elenodes_without_children_count(sqls, &pool).await {
        let mut result = HashMap::new();
        for ele in elenodes {
            if let Some(room_name) = refno_room_map.get(&ele.refno) {
                result.insert(ele.refno, SscEleNode {
                    refno: ele.refno,
                    noun: ele.noun,
                    name: ele.name,
                    owner: ele.owner,
                    room_code: room_name.value().to_string(),
                });
            }
        }
        println!("总共有{}房间元件", result.len());
        return Ok(result);
    }
    Ok(HashMap::default())
}

pub async fn query_all_room_data_aql(database: &ArDatabase, pool: &Pool<MySql>, db_option: &DbOption) -> anyhow::Result<HashMap<RefU64, SscEleNode>> {
    let mut result = HashMap::new();
    let all_room = parse_room_info_from_excel()?;
    let room_map = query_all_need_compute_room_refno(&db_option.clone().arch_db_nums.unwrap_or_default(), "FRMW", Some("-RM"), pool).await?;
    for (room_refno, room_name) in room_map.iter() {
        // 通过命名规则获取到需要的房间名
        let room_info = get_room_name_split(room_name);
        if room_info.is_none() { continue; }
        let room_info = room_info.unwrap();
        if let Some(room_map) = all_room.get(&room_info.factory) {
            if let Some(rooms) = room_map.get(&room_info.leave) {
                let room_name = format!("1{}", &room_info.room_name);
                if rooms.contains(&room_name) {
                    // 找到房间下所有的元件
                    result.entry(*room_refno).or_insert(SscEleNode{
                        refno: Default::default(),
                        noun: "".to_string(),
                        name: "".to_string(),
                        owner: Default::default(),
                        room_code: room_name,
                    });
                    // let panes = travel_children_with_type(*room_refno,"PANE".to_string(),pool).await?;
                    // let target_refnos = get_room_refnos_from_spa_tree_aql(*room_refno, &database).await?;
                    // if let Ok(elenodes) = query_elenodes_without_children_count(target_refnos, &pool).await {
                    //     for ele in elenodes {
                    //         result.entry(ele.refno).or_insert(SscEleNode {
                    //             refno: ele.refno,
                    //             noun: ele.noun,
                    //             name: ele.name,
                    //             owner: ele.owner,
                    //             room_code: room_info.room_name.to_string(),
                    //         });
                    //     }
                    // }
                }
            }
        }
    }
    Ok(result)
}

pub async fn query_ssc_children(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Vec<EleTreeNode>> {
    let sql = gen_query_ssc_children_sql(refno);
    let result = sqlx::query(&sql).fetch_all(pool).await;
    return match result {
        Ok(vals) => {
            let mut r = vec![];
            for val in vals {
                let refno = RefU64(val.get::<i64, _>("ID") as u64);
                let children_count = query_ssc_children_count(refno, &pool).await?;
                let node = EleTreeNode {
                    refno,
                    noun: val.get::<String, _>("TYPE"),
                    name: val.get::<String, _>("NAME"),
                    owner: RefU64(val.get::<i64, _>("OWNER") as u64),
                    children_count,
                };
                r.push(node);
            }
            Ok(r)
        }
        Err(e) => {
            dbg!(sql);
            dbg!(e);
            Ok(vec![])
        }
    };
}

pub async fn query_ssc_children_without_children_count(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Vec<EleTreeNode>> {
    let sql = gen_query_ssc_children_sql(refno);
    let result = sqlx::query(&sql).fetch_all(pool).await;
    return match result {
        Ok(vals) => {
            let mut r = vec![];
            for val in vals {
                let refno = RefU64(val.get::<i64, _>("ID") as u64);
                let node = EleTreeNode {
                    refno,
                    noun: val.get::<String, _>("TYPE"),
                    name: val.get::<String, _>("NAME"),
                    owner: RefU64(val.get::<i64, _>("OWNER") as u64),
                    children_count: 0,
                };
                r.push(node);
            }
            Ok(r)
        }
        Err(e) => {
            dbg!(sql);
            dbg!(e);
            Ok(vec![])
        }
    };
}

pub async fn query_ssc_world(pool: &Pool<MySql>) -> anyhow::Result<Option<EleTreeNode>> {
    let sql = gen_query_ssc_world_sql();
    let result = sqlx::query(&sql).fetch_one(pool).await;
    return match result {
        Ok(val) => {
            let refno = RefU64(val.get::<i64, _>("ID") as u64);
            let children_count = query_ssc_children_count(refno, &pool).await?;
            let node = EleTreeNode {
                refno,
                noun: val.get::<String, _>("TYPE"),
                name: val.get::<String, _>("NAME"),
                owner: RefU64(val.get::<i64, _>("OWNER") as u64),
                children_count,
            };
            Ok(Some(node))
        }
        Err(e) => {
            dbg!(sql);
            dbg!(e);
            Ok(None)
        }
    };
}

/// 查找ssc的owner
pub async fn query_ssc_owner(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Option<RefU64>> {
    let sql = gen_query_ssc_owner_sql(refno);
    let result = sqlx::query(&sql).fetch_one(pool).await;
    return match result {
        Ok(r) => {
            Ok(Some(RefU64(r.get::<i64, _>("OWNER") as u64)))
        }
        Err(e) => {
            dbg!(&sql);
            dbg!(&e);
            Ok(None)
        }
    };
}

/// 查找ssc的type
pub async fn query_ssc_type(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Option<String>> {
    let sql = gen_query_ssc_type_sql(refno);
    let result = sqlx::query(&sql).fetch_one(pool).await;
    return match result {
        Ok(r) => {
            Ok(Some(r.get::<String, _>("TYPE")))
        }
        Err(e) => {
            dbg!(&sql);
            dbg!(&e);
            Ok(None)
        }
    };
}


/// 获取children有那些tpe
pub async fn query_ssc_children_contains_types(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Option<Vec<String>>> {
    if let Ok(children) = query_ssc_children_without_children_count(refno, pool).await {
        let result = children.into_iter().map(|child| {
            child.noun
        }).collect::<Vec<String>>();
        return Ok(Some(result));
    }
    Ok(None)
}

pub async fn query_ssc_children_count(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<usize> {
    let count_sql = gen_query_ssc_children_count_sql(refno);
    let count_result = sqlx::query(&count_sql).fetch_one(pool).await?;
    Ok(count_result.get::<i32, _>(0) as usize)
}

/// 遍历该ssc节点的所有子节点
pub async fn travel_ssc_children(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Vec<RefU64>> {
    let mut result = vec![];
    let mut deque = VecDeque::new();
    deque.push_back(refno);
    result.push(refno);
    while deque.len() > 0 {
        let refno = deque.pop_front().unwrap();
        let children = query_ssc_children_without_children_count(refno, pool).await?;
        for child in children {
            deque.push_back(child.refno);
            result.push(child.refno);
        }
    }
    Ok(result)
}

pub async fn get_ancestor_till_type(mut refno: RefU64, att_type: Option<&str>, pool: &Pool<MySql>) -> anyhow::Result<Option<RefU64>> {
    if let Some(att_type) = att_type {
        let mut cur_owner_type = "".to_string();
        while att_type != &cur_owner_type {
            if let Some(v) = SSC_OWNER_MAP.get(&refno) {
                refno = v.value().0;
                cur_owner_type = v.value().1.to_string();
            } else {
                if let Some(owner) = query_ssc_owner(refno, pool).await? {
                    if refno == owner {
                        break;
                    }
                    if let Some(owner_type) = query_ssc_type(owner, pool).await? {
                        SSC_OWNER_MAP.insert(refno, (owner, owner_type.clone()));
                        refno = owner;
                        cur_owner_type = owner_type;
                    }
                } else {
                    break;
                }
            }
        }
        if att_type == cur_owner_type {
            return Ok(Some(refno));
        }
    }
    Ok(None)
}

pub async fn update_ssc_type(names: Vec<String>, pool: &Pool<MySql>) -> anyhow::Result<()> {
    let mut insert_sql = String::new();
    for name in names {
        insert_sql.push_str(&format!("'{}',", name));
    }
    insert_sql.remove(insert_sql.len() - 1);
    let sql = gen_update_ssc_type_sql(insert_sql, "SSC_ROOM");
    let result = pool.execute(sql.as_str()).await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(e);
            dbg!(sql.as_str());
        }
    }
    Ok(())
}


fn gen_query_ssc_children_count_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select count(*) from {PDMS_SSC_ELEMENTS_TABLE} where owner = {}", refno.0));
    sql
}

fn gen_query_ssc_children_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select * from {PDMS_SSC_ELEMENTS_TABLE} where owner = {}", refno.0));
    sql
}

fn gen_query_ssc_world_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select * from {PDMS_SSC_ELEMENTS_TABLE} where type = 'WORL' ;"));
    sql
}

fn gen_query_all_room_data_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select * from {ROOM_CODE}"));
    sql
}

fn gen_get_refno_by_name_sql(name: String) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID FROM {PDMS_SSC_ELEMENTS_TABLE} WHERE NAME = '{}' ", name));
    sql
}

fn gen_query_ssc_owner_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT OWNER FROM {PDMS_SSC_ELEMENTS_TABLE} WHERE ID = {} ", refno.0));
    sql
}

fn gen_query_ssc_type_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT TYPE FROM {PDMS_SSC_ELEMENTS_TABLE} WHERE ID = {} ", refno.0));
    sql
}

fn gen_update_ssc_type_sql(name: String, change_type: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("UPDATE {PDMS_SSC_ELEMENTS_TABLE} SET TYPE = '{}' WHERE NAME IN ({})", change_type, name));
    sql
}

#[tokio::test]
async fn test_get_ancestor_refnos() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let v = get_ancestor_till_type(RefI32Tuple((0, 6)).into(), Some("WORL"), &pool).await?;
    println!("v={:?}", v);
    Ok(())
}