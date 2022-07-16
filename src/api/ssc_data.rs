use std::collections::VecDeque;
use std::sync::Arc;
use aios_core::pdms_types::{EleTreeNode, RefU64};
use dashmap::{DashMap, DashSet};
use sqlx::{MySql, Pool, Row};
use crate::consts::ROOM_CODE;
use serde::{Serialize, Deserialize};
use crate::consts::PDMS_SSC_ELEMENTS_TABLE;
use crate::api::element::{query_ele_node, query_elenode_without_children_count, query_elenodes_without_children_count};

#[derive(Debug, Default, Eq, PartialEq, Serialize, Deserialize, Hash)]
pub struct SscEleNode {
    pub refno: RefU64,
    pub noun: String,
    pub name: String,
    pub owner: RefU64,
    pub room_code: String,
}

/// 获取所有带有房间号的节点属性
pub async fn query_all_room_data(pool: &Pool<MySql>) -> anyhow::Result<Vec<SscEleNode>> {
    let sql = gen_query_all_room_data_sql();
    let vals = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    let mut refno_room_map = DashMap::new();
    let mut sql = String::new();
    let mut sqls = vec![];
    for val in vals {
        let refno = RefU64(val.get::<i64, _>("REFNO") as u64);
        let room_name = val.get::<String, _>("ROOM_NAME");
        refno_room_map.insert(refno, room_name);
        sqls.push(refno);
    }
    if let Ok(elenodes) = query_elenodes_without_children_count(sqls, &pool).await {
        let mut result = vec![];
        for ele in elenodes {
            if let Some(room_name) = refno_room_map.get(&ele.refno) {
                result.push(SscEleNode {
                    refno: ele.refno,
                    noun: ele.noun,
                    name: ele.name,
                    owner: ele.owner,
                    room_code: room_name.value().to_string(),
                })
            }
        }
        println!("总共有{}房间元件", result.len());
        return Ok(result);
    }
    Ok(vec![])
}

pub async fn query_ssc_children(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Vec<EleTreeNode>> {
    let sql = gen_query_ssc_children_sql(refno);
    let result = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await;
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
    let result = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await;
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
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
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
    let count_result = sqlx::query(&count_sql).fetch_one(&mut pool.acquire().await?).await?;
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
        for child in children{
            deque.push_back(child.refno);
            result.push(child.refno);
        }
    }
    Ok(result)
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