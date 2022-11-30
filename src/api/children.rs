use std::collections::VecDeque;
use std::env;
use std::fmt::format;
use std::sync::Arc;
use aios_core::pdms_types::*;
use arangors_lite::{Connection, Database};
use calamine::Error::De;
use dashmap::DashSet;
use sqlx::{Error, MySql, Pool, Row};
use crate::consts::PDMS_ELEMENTS_TABLE;
use crate::api::element::*;
use crate::data_interface::tidb_manager::AiosDBManager;
use serde::{Serialize, Deserialize};
use sqlx::mysql::MySqlRow;
use crate::aql_api::children::query_owner_with_type_aql;
use crate::defines::{AiosString, CACHED_MDB_SITE_MAP};

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


/// 遍历该节点的children，不包含叶子节点
pub async fn travel_children_without_leaf(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Vec<RefU64>> {
    let mut result = vec![];
    let mut deque = VecDeque::new();
    deque.push_back(refno);
    result.push(refno);
    while deque.len() > 0 {
        let refno = deque.pop_front().unwrap();
        let children = query_children_eles(refno, pool).await?;
        for ele in children {
            if ele.children_count != 0 {
                if let Ok(ele_refno) = RefU64::from_refno_str(ele.refno.as_str()) {
                    result.push(ele_refno);
                    deque.push_back(ele_refno);
                }
            }
        }
    }
    Ok(result)
}

/// 遍历该节点下的 children (不包含自己) 返回 EleTreeNode
pub async fn travel_children_for_elenode(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Vec<EleTreeNode>> {
    let mut result = vec![];
    let mut deque = VecDeque::new();
    deque.push_back(refno);
    while deque.len() > 0 {
        let refno = deque.pop_front().unwrap();
        let children = query_children_eles(refno, pool).await?;
        for ele in children {
            if let Ok(refno) = RefU64::from_refno_str(ele.refno.as_str()) {
                deque.push_back(refno);
                result.push(EleTreeNode {
                    refno,
                    noun: ele.noun,
                    name: ele.name,
                    owner: ele.owner,
                    children_count: ele.children_count,
                })
            }
        }
    }
    Ok(result)
}

pub async fn travel_children_for_elenode_without_children_count(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Vec<EleTreeNode>> {
    let mut result = vec![];
    let mut deque = VecDeque::new();
    deque.push_back(refno);
    while deque.len() > 0 {
        let refno = deque.pop_front().unwrap();
        let children = query_children_eles_without_children_count(refno, pool).await?;
        for ele in children {
            if let Ok(refno) = RefU64::from_refno_str(ele.refno.as_str()) {
                deque.push_back(refno);
                result.push(EleTreeNode {
                    refno,
                    noun: ele.noun,
                    name: ele.name,
                    owner: ele.owner,
                    children_count: ele.children_count,
                })
            }
        }
    }
    Ok(result)
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SimpleNodeDataForPlat {
    pub refno: String,
    pub name: String,
    pub owner: String,
}

/// 遍历该节点的所有子节点为指定type的所有数据 返回 refno name owner
pub async fn travel_children_with_type(refno: RefU64, att_type: String, pool: &Pool<MySql>) -> anyhow::Result<Vec<EleTreeNode>> {
    let mut result = vec![];
    let children = travel_children_eles(refno, pool).await?;
    let sql = gen_query_names_from_refnos_with_type_sql(children, att_type);
    let vals = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for val in vals {
        let refno = RefU64(val.get::<i64, _>("ID") as u64);
        let name = val.get::<String, _>("NAME");
        let owner = RefU64(val.get::<i64, _>("OWNER") as u64);
        result.push(EleTreeNode {
            refno,
            name,
            owner,
            ..Default::default()
        });
    }
    Ok(result)
}


/// 遍历该节点的所有子节点为指定refno的所有数据 返回 refno name owner
pub async fn travel_children_with_refno(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Vec<RefU64>> {
    let children = travel_children_eles(refno, pool).await?;
    Ok(children)
}

/// 查询指定type的children 的 id 、 name
pub async fn query_children_id_name_with_type(refno: RefU64, att_type: &str, pool: &Pool<MySql>) -> anyhow::Result<Vec<(RefU64, String)>> {
    let mut result = vec![];
    let sql = gen_query_children_id_name_with_type_sql(refno, att_type);
    let vals = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for val in vals {
        let child_refno = RefU64(val.get::<i64, _>("ID") as u64);
        let name = val.get::<String, _>("NAME");
        result.push((child_refno, name));
    }
    Ok(result)
}

/// 模糊查询 类型为 att_type ，name 中包含指定值 的所有 refno和 name
pub async fn fuzzy_query_refnos_by_name(att_type: String, name: String, pool: &Pool<MySql>) -> anyhow::Result<Vec<(RefU64, String)>> {
    let mut result = vec![];
    let att_type = if att_type == "\"\"" { None } else { Some(att_type) };
    let sql = gen_fuzzy_query_refnos_by_name_sql(att_type, name);
    let vals = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for val in vals {
        let refno = RefU64(val.get::<i64, _>("ID") as u64);
        let name = val.get::<String, _>("NAME");
        result.push((refno, name));
    }
    Ok(result)
}

pub async fn fuzzy_query_refnos_by_name_limit(name: String, numbdbs: &Vec<i32>, pool: &Pool<MySql>) -> anyhow::Result<Vec<(RefU64, String)>> {
    let mut result = vec![];
    let sql = gen_fuzzy_query_refnos_by_name_sql_limit(name, numbdbs);
    let vals = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for val in vals {
        let refno = RefU64(val.get::<i64, _>("ID") as u64);
        let name = val.get::<String, _>("NAME");
        result.push((refno, name));
    }
    Ok(result)
}

/// 获取参考号集合属于哪些 numbdb
pub async fn query_numbdb_from_refnos(refnos: Vec<RefU64>, pool: &Pool<MySql>) -> anyhow::Result<Vec<i32>> {
    if refnos.is_empty() { return Ok(vec![]); }
    let sql = gen_query_numbdb_from_refnos(refnos);
    let val = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await;
    return match val {
        Ok(vals) => {
            let mut result = vec![];
            for val in vals {
                result.push(val.get::<i32, _>("NUMBDB"));
            }
            Ok(result)
        }
        Err(e) => {
            dbg!(&e);
            Ok(vec![])
        }
    };
}

/// 获取参考号属于那个dbnum
pub async fn query_numbdb_by_refno(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<i32> {
    let sql = gen_query_numbdb_by_refno(refno);
    let val = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    Ok(val.try_get::<i32, _>("NUMBDB")?)
}

/// 通过 refno 获取 owner 和 owner 的type
pub async fn query_owner_type_from_id(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Option<(RefU64, String)>> {
    if let Ok(Some(owner)) = query_owner_from_id(refno, pool).await {
        if let Ok(att_type) = query_refno_type(owner, pool).await {
            return Ok(Some((owner, att_type)));
        }
    }
    Ok(None)
}

pub async fn query_ancestor_of_type(mut refno: RefU64, att_type: &str, pool: &Pool<MySql>) -> anyhow::Result<Option<RefU64>> {
    while let Some((owner_refno, owner_type)) = query_owner_type_from_id(refno, pool).await? {
        refno = owner_refno;
        if owner_type == att_type {
            break;
        }
    }
    Ok(Some(refno))
}

pub async fn query_ancestor_refnos_till_type(mut refno: RefU64, att_type: &str, pool: &Pool<MySql>) -> anyhow::Result<Vec<RefU64>> {
    let mut result = vec![];
    while let Some((owner_refno, owner_type)) = query_owner_type_from_id(refno, pool).await? {
        result.push(refno);
        refno = owner_refno;
        if owner_type == att_type {
            result.push(owner_refno);
            break;
        }
    }
    Ok(result)
}

pub async fn query_ancestor_refnos_till_type_aql(mut refno: RefU64, att_type: &str, mgr: Arc<AiosDBManager>) -> anyhow::Result<Vec<RefU64>> {
    let database = mgr.get_arangodb_conn().await?;
    let mut result = vec![];
    while let Some((owner_refno, owner_type)) = query_owner_with_type_aql(&database, refno).await? {
        result.push(refno);
        refno = owner_refno;
        if owner_type == att_type {
            result.push(owner_refno);
            break;
        }
    }
    Ok(result)
}

/// 获取children有那些tpe
pub async fn query_children_contains_types(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Option<Vec<String>>> {
    if let Ok(children) = query_children_eles(refno, pool).await {
        let result = children.into_iter().map(|child| {
            child.noun
        }).collect::<Vec<String>>();
        return Ok(Some(result));
    }
    Ok(None)
}

/// 查找他的owner直到包含传进来的Vec<att_type>
pub async fn query_owner_till_type(mut refno: RefU64, types: Vec<String>, pool: &Pool<MySql>) -> anyhow::Result<RefU64> {
    while let Some((owner_refno, owner_type)) = query_owner_type_from_id(refno, pool).await? {
        refno = owner_refno;
        if types.contains(&owner_type) {
            break;
        }
    }
    Ok(refno)
}

/// 将树节点的 site 提前保存下来
pub async fn cache_site_node(mdb: &str, module: &str, pool: &Pool<MySql>) {
    if let Ok(world) = query_world(mdb, module, pool).await {
        if !CACHED_MDB_SITE_MAP.contains_key(&world.refno) {
            if let Ok(mut children) = query_world_children_eles(mdb, module, pool).await {
                for mut child in &mut children {
                    child.owner = world.refno;
                }
                CACHED_MDB_SITE_MAP.insert(world.refno, &PdmsElementVec(children));
            }
        }
    }
}

pub async fn cache_mdb_module_numbdbs(mdb: &str, module: &str, pool: &Pool<MySql>) -> anyhow::Result<Vec<i32>> {
    if let Ok(world) = query_world(mdb, module, pool).await {
        if CACHED_MDB_SITE_MAP.contains_key(&world.refno) {
            let children = CACHED_MDB_SITE_MAP.get(&world.refno).unwrap();
            let children = children.value().iter()
                .map(|x| RefU64::from_refno_str(&x.refno).unwrap_or(RefU64(0))).collect::<Vec<RefU64>>();
            let result = query_numbdb_from_refnos(children, pool).await?;
            return Ok(result);
        }
    }
    Ok(vec![])
}

/// 找到某numbdb下的所有指定类型的参考号
pub async fn query_type_refnos_by_numbdb(numbdb: i32, att_type: String, pool: &Pool<MySql>) -> anyhow::Result<Vec<RefU64>> {
    let mut result = vec![];
    let sql = gen_query_refnos_by_numbdb(numbdb, att_type);
    let vals = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for val in vals {
        let refno = val.get::<i64, _>("ID");
        result.push(RefU64(refno as u64));
    }
    Ok(result)
}

pub async fn query_contain_noun_refnos(noun: String, pool: &Pool<MySql>) -> anyhow::Result<DashSet<String>> {
    let mut result = DashSet::new();
    let sql = gen_query_contain_noun_refnos(noun);
    let vals = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for val in vals {
        let table = val.get::<String, _>("TABLE_NAME").to_uppercase();
        result.insert(table);
    }
    Ok(result)
}

/// 查找整张表的外键属性 返回值 ； 0 ： 自身 refno   1： 外键 refno
pub async fn query_foreign_refnos_from_table(foreign_type: &str, table_name: &str, pool: &Pool<MySql>) -> anyhow::Result<Vec<(RefU64, RefU64)>> {
    let mut result = Vec::new();
    let sql = gen_query_foreign_refnos_from_table_sql(foreign_type, table_name);
    let vals = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for val in vals {
        let refno = RefU64(val.get::<i64, _>("ID") as u64);
        let foreign_refno = RefU64(val.get::<i64, _>(foreign_type) as u64);
        result.push((refno, foreign_refno));
    }
    Ok(result)
}


fn gen_query_names_from_refnos_with_type_sql(refnos: Vec<RefU64>, att_type: String) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID,NAME,OWNER FROM {PDMS_ELEMENTS_TABLE} WHERE TYPE = '{}' AND ID IN ( ", att_type));
    let mut insert_sql = String::new();
    for refno in refnos {
        insert_sql.push_str(&format!("{},", refno.0));
    }
    insert_sql.remove(insert_sql.len() - 1);
    sql.push_str(&format!("{} );", insert_sql));
    sql
}

fn gen_query_names_from_refnos_sql(refnos: Vec<RefU64>) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID,NAME,OWNER FROM {PDMS_ELEMENTS_TABLE} WHERE ID IN ( "));
    let mut insert_sql = String::new();
    for refno in refnos {
        insert_sql.push_str(&format!("{},", refno.0));
    }
    insert_sql.remove(insert_sql.len() - 1);
    sql.push_str(&format!("{} );", insert_sql));
    sql
}

fn gen_query_names_from_refnos_with_name_sql(refnos: Vec<RefU64>, name: String) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID,NAME,OWNER FROM {PDMS_ELEMENTS_TABLE} WHERE NAME = '{}' IN ( ", name));
    let mut insert_sql = String::new();
    for refno in refnos {
        insert_sql.push_str(&format!("{},", refno.0));
    }
    insert_sql.remove(insert_sql.len() - 1);
    sql.push_str(&format!("{} );", insert_sql));
    sql
}

fn gen_query_foreign_refnos_from_table_sql(foreign_type: &str, table_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID,{} FROM {}", foreign_type, table_name));
    sql
}

fn gen_query_children_id_name_with_type_sql(refno: RefU64, att_type: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID,NAME FROM {PDMS_ELEMENTS_TABLE} WHERE OWNER = {} AND TYPE = '{}' ", refno.0, att_type));
    sql
}

fn gen_fuzzy_query_refnos_by_name_sql(att_type: Option<String>, name: String) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID,NAME FROM {PDMS_ELEMENTS_TABLE} WHERE NAME LIKE '%{}%' ", name));
    if att_type.is_some() {
        sql.push_str(&format!("AND TYPE = '{}' ", att_type.unwrap()))
    }
    sql
}

fn gen_fuzzy_query_refnos_by_name_sql_limit(name: String, numbdbs: &Vec<i32>) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID,NAME FROM {PDMS_ELEMENTS_TABLE} WHERE NAME LIKE '%{}%' ", name));
    if !numbdbs.is_empty() {
        sql.push_str("AND NUMBDB IN (");
    }
    for numbdb in numbdbs {
        sql.push_str(&format!("{} ,", numbdb.to_string()));
    }
    if !numbdbs.is_empty() {
        sql.remove(sql.len() - 1);
        sql.push_str(")");
    }
    sql.push_str("LIMIT 10");
    sql
}

fn gen_query_owner_type_from_id(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT OWNER,TYPE FROM {PDMS_ELEMENTS_TABLE} WHERE ID = {} AND IS_DEL = 0 ", refno.0));
    sql
}

fn gen_query_refnos_by_numbdb(numbdb: i32, att_type: String) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID FROM {PDMS_ELEMENTS_TABLE} WHERE NUMBDB = {} AND TYPE = '{}' ", numbdb, att_type));
    sql
}

fn gen_query_contain_noun_refnos(noun: String) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT TABLE_NAME FROM information_schema.columns WHERE column_name='{}'", noun));
    sql
}

fn gen_query_numbdb_from_refnos(refnos: Vec<RefU64>) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT NUMBDB FROM {PDMS_ELEMENTS_TABLE} WHERE ID IN (", ));
    for refno in refnos {
        sql.push_str(&format!("{} ,", refno.0.to_string()));
    }
    sql.remove(sql.len() - 1);
    sql.push_str(")");
    sql
}

fn gen_query_numbdb_by_refno(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT NUMBDB FROM {PDMS_ELEMENTS_TABLE} WHERE ID = {}", refno.0));
    sql
}

#[tokio::test]
async fn test_travel_children_eles() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let refno: RefU64 = RefI32Tuple((23584, 5693)).into();
    let v = travel_children_eles(refno, &pool).await?;
    dbg!(&v);
    Ok(())
}

#[tokio::test]
async fn test_query_ancestor_of_type() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let refno: RefU64 = RefI32Tuple((23584, 38)).into();
    let v = query_ancestor_of_type(refno, "SITE", &pool).await?;
    println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_query_contain_noun_refnos() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let v = query_contain_noun_refnos("SPRE".to_string(), &pool).await?;
    println!("v={:?}", v);
    Ok(())
}