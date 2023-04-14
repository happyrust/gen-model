use std::collections::{BTreeMap, HashMap};
use std::env;

use aios_core::pdms_types::*;
use aios_core::tool::db_tool::db1_hash;
use anyhow::anyhow;
use dashmap::DashMap;
use futures::poll;
use itertools::Itertools;
use log::info;
use parse_pdms_db::parse::WholeAttMap;
use smol_str::SmolStr;
use sqlx::{Error, MySql, Pool, Row};
// use sea_orm::sea_query::any;
use sqlx::mysql::{MySqlQueryResult, MySqlRow};

use crate::api::attr::{query_explicit_attr, query_implicit_attr};
use crate::api::children::{query_db_num_by_refno, query_numbdb_from_refnos};
use crate::api::dbno_sql::{query_dbno_count};
use crate::api::project_mdb::*;
use crate::api::test_sample::{get_test_info_pool, get_test_sample_pool};
use crate::consts::*;
use crate::data_interface::tidb_manager::AiosDBManager;

pub const ATT_DIVCO: i32 = 688051937;

/// 通过 refno 返回对应的 type
pub async fn query_refno_type(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<String> {
    let sql = gen_query_refno_type_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    Ok(result.get::<String, _>(0))
}

pub async fn query_children_pdms_tree(mdb: &str, model: &str, refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Vec<(RefU64, String)>> {
    let type_name = query_refno_type(refno, &pool).await?;
    return if type_name == "WORL" {
        query_world_children(mdb, model, pool).await
    } else {
        query_children(refno, pool).await
    };
}


///获取当前world下所有的子节点
pub async fn query_world_children(mdb: &str, model: &str, pool: &Pool<MySql>) -> anyhow::Result<Vec<(RefU64, String)>> {
    let mut result = vec![];
    let mdb = format!("/{}", mdb);
    let world_refnos = query_world_refnos(&mdb, model, pool).await?;
    for world in world_refnos {
        let children = query_children(world, pool).await?;
        result.push(children);
    }
    Ok(result.into_iter().flatten().collect())
}

/// 获取world下的pdms elements
pub async fn query_world_children_eles(mdb: &str, model: &str, pool: &Pool<MySql>) -> anyhow::Result<Vec<PdmsElement>> {
    let mut result = vec![];
    let mdb = format!("/{mdb}");
    let world_refnos = query_world_refnos(&mdb, model, pool).await?;
    for world in world_refnos {
        let children = query_children_eles(world, pool).await?;
        result.push(children);
    }
    Ok(result.into_iter().flatten().collect())
}

/// 获取某个refno 的 children 并未合并 world
pub async fn query_children(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Vec<(RefU64, String)>> {
    let mut r = vec![];
    let mut b_map = BTreeMap::new();
    let sql = gen_pdms_elements_get_children_ele_node_sql(refno);
    let vals = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for val in vals {
        let child_refno = RefU64(val.get::<i64, _>("ID") as u64);
        let name = val.get::<String, _>("NAME");
        let order = val.get::<i32, _>("ORDER_NUM");
        b_map.insert(order, (child_refno, name));
    }
    for (_, v) in b_map {
        r.push(v);
    }
    Ok(r)
}

pub async fn query_children_eles(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Vec<PdmsElement>> {
    let mut r = vec![];
    let mut b_map = BTreeMap::new();
    let sql = gen_pdms_elements_get_children_ele_node_sql(refno);
    let vals = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for val in vals {
        let child_refno = RefU64(val.get::<i64, _>("ID") as u64);
        let name = val.get::<String, _>("NAME");
        let type_name = val.get::<String, _>("TYPE");
        let owner = RefU64(val.get::<i64, _>("OWNER") as u64);
        let order = val.get::<i32, _>("ORDER_NUM");
        let children_count = val.get::<i32, _>("CHILDREN_COUNT");
        b_map.insert(order, PdmsElement {
            refno: child_refno.to_string(),
            owner,
            name,
            noun: type_name,
            version: 0,
            children_count: children_count as usize,
        });
    }
    for (_, v) in b_map {
        r.push(v);
    }
    Ok(r)
}

pub async fn query_children_eles_without_children_count(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Vec<PdmsElement>> {
    let mut r = vec![];
    let mut b_map = BTreeMap::new();
    let sql = gen_pdms_elements_get_children_ele_node_sql(refno);
    let vals = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for val in vals {
        let child_refno = RefU64(val.get::<i64, _>("ID") as u64);
        let name = val.get::<String, _>("NAME");
        let type_name = val.get::<String, _>("TYPE");
        let owner = RefU64(val.get::<i64, _>("OWNER") as u64);
        let order = val.get::<i32, _>("ORDER_NUM");
        b_map.insert(order, PdmsElement {
            refno: child_refno.to_string(),
            owner,
            name,
            noun: type_name,
            version: 0,
            children_count: 0,
        });
    }
    for (_, v) in b_map {
        r.push(v);
    }
    Ok(r)
}

pub async fn query_world(mdb: &str, module: &str, pool: &Pool<MySql>) -> anyhow::Result<EleTreeNode> {
    let mdb = format!("/{}", mdb);
    let world_refnos = query_world_refnos(&mdb, module, pool).await?;
    let refno = world_refnos.iter().next().ok_or(anyhow!("Not exist world refno"))?;
    query_ele_node(*refno, pool).await
}

/// 查询生成Element node
pub async fn query_ele_node(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<EleTreeNode> {
    let sql = format!("SELECT * FROM {PDMS_ELEMENTS_TABLE} WHERE ID = {} and IS_DEL = 0;", *refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    Ok(EleTreeNode {
        refno,
        noun: result.get::<String, _>("TYPE"),
        name: result.get::<String, _>("NAME"),
        owner: RefU64::from(result.get::<i64, _>("OWNER") as u64),
        children_count: result.get::<i32, _>("CHILDREN_COUNT") as usize,
    })
}

pub async fn query_ele_nodes_by_refnos(refnos: Vec<RefU64>, pool: &Pool<MySql>) -> anyhow::Result<Vec<EleTreeNode>> {
    let mut refno_sql = String::new();
    for refno in refnos {
        refno_sql.push_str(&format!("{},", refno.0));
    }
    if refno_sql.is_empty() { return Ok(vec![]); }
    refno_sql.remove(refno_sql.len() - 1);
    let sql = format!("SELECT ID,TYPE,NAME,OWNER,CHILDREN_COUNT FROM {PDMS_ELEMENTS_TABLE} WHERE ID in ({}) and IS_DEL = 0;", refno_sql);
    let results = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    let mut nodes = Vec::new();
    for result in results {
        let refno = result.get::<i64, _>("ID");
        let noun = result.get::<String, _>("TYPE");
        let name = result.get::<String, _>("NAME");
        let owner = RefU64::from(result.get::<i64, _>("OWNER") as u64);
        let children_count = result.get::<i32, _>("CHILDREN_COUNT") as usize;
        nodes.push(EleTreeNode {
            refno: RefU64(refno as u64),
            noun,
            name,
            owner,
            children_count,
        });
    }
    Ok(nodes)
}

/// 查询生成Element node ,不查询children_count 默认为 0
pub async fn query_elenode_without_children_count(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<EleTreeNode> {
    let sql = format!("SELECT TYPE,NAME,OWNER FROM {PDMS_ELEMENTS_TABLE} WHERE ID = {} and IS_DEL = 0;", *refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    Ok(EleTreeNode {
        refno,
        noun: result.get::<String, _>("TYPE"),
        name: result.get::<String, _>("NAME"),
        owner: RefU64::from(result.get::<i64, _>("OWNER") as u64),
        children_count: 0,
    })
}

pub async fn query_elenodes_without_children_count(refnos: Vec<RefU64>, pool: &Pool<MySql>) -> anyhow::Result<Vec<EleTreeNode>> {
    let mut eles = vec![];
    let mut sqls = vec![];
    for refs in refnos.chunks(10000) {
        let mut refno_str = String::new();
        for refno in refs {
            refno_str.push_str(&format!("{} ,", refno.0));
        }
        refno_str.remove(refno_str.len() - 1);
        let sql = format!("SELECT ID,TYPE,NAME,OWNER FROM {PDMS_ELEMENTS_TABLE} WHERE ID IN ( {} ) and IS_DEL = 0;", refno_str);
        sqls.push(sql);
    }
    for sql in sqls {
        let result = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
        for r in result {
            eles.push(EleTreeNode {
                refno: RefU64(r.get::<i64, _>("ID") as u64),
                noun: r.get::<String, _>("TYPE"),
                name: r.get::<String, _>("NAME"),
                owner: RefU64::from(r.get::<i64, _>("OWNER") as u64),
                children_count: 0,
            })
        }
    }
    Ok(eles)
}

pub async fn query_world_ele_node(mdb: &str, module: &str, pool: &Pool<MySql>, mgr: &AiosDBManager) -> anyhow::Result<Option<PdmsElement>> {
    let mdb = format!("/{}", mdb);
    //需要在这里判断是哪个 project pool
    let quicks = query_db_quick_info(&mdb, module, &pool).await?;
    let quick = &quicks[0];
    let sql = gen_query_node_id_from_refno_sql(quick.world_refno);
    let world_pool = mgr.get_project_pool(&quick.project).ok_or(anyhow!("project not found"))?;
    let result = sqlx::query(&sql).fetch_one(&mut world_pool.acquire().await?).await;
    return match result {
        Ok(val) => {
            let owner = RefU64(val.get::<i64, _>("OWNER") as u64);
            let name = val.get::<String, _>("NAME");
            let type_name = val.get::<String, _>("TYPE");
            let children_count = val.get::<i32, _>("CHILDREN_COUNT") as usize;
            Ok(Some(PdmsElement {
                refno: quick.world_refno.to_string(),
                owner,
                name,
                noun: type_name,
                version: 0,
                children_count,
            }))
        }
        Err(e) => {
            dbg!(&quick);
            dbg!(sql);
            Ok(None)
        }
    };
}

/// 通过 refno 获取 owner
pub async fn query_owner_from_id(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Option<RefU64>> {
    let sql = gen_query_owner_from_id(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    return match result {
        Ok(v) => { Ok(Some(RefU64(v.get::<i64, _>(0) as u64))) }
        Err(_) => { Ok(None) }
    };
}

/// 通过 name 获取 refno （pdms）
pub async fn query_id_from_name(name: &str, att_type: Option<String>, pool: &Pool<MySql>) -> anyhow::Result<Vec<RefU64>> {
    let mut r = vec![];
    let sql = gen_query_id_from_name_sql(name, att_type);
    let results = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await;
    if let Ok(results) = results {
        for result in results {
            r.push(RefU64(result.get::<i64, _>(0) as u64))
        }
    }
    Ok(r)
}

/// 通过 name 获取 refno （ssc）
pub async fn query_id_from_name_ssc(name: &str, pool: Pool<MySql>) -> anyhow::Result<Option<RefU64>> {
    let sql = gen_query_id_from_name_ssc_sql(name);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    match result {
        Ok(v) => {
            let refno = RefU64(v.get::<i64, _>("ID") as u64);
            Ok(Some(refno))
        }
        Err(_) => { Ok(None) }
    }
}

fn gen_query_pdms_elements_type_name_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT TYPE FROM {PDMS_ELEMENTS_TABLE} WHERE ID = {} AND IS_DEL = 0 ", refno.0));
    sql
}

fn gen_query_owner_from_id(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT OWNER FROM {PDMS_ELEMENTS_TABLE} WHERE ID = {} AND IS_DEL = 0 ", refno.0));
    sql
}

fn gen_query_id_from_name_sql(name: &str, att_type: Option<String>) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID FROM {PDMS_ELEMENTS_TABLE} WHERE NAME like '%{}%' ", name));
    if let Some(att_type) = att_type {
        sql.push_str(&format!("AND TYPE = '{}' ", att_type));
    }
    sql
}

fn gen_query_id_from_name_ssc_sql(name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID FROM {PDMS_SSC_ELEMENTS_TABLE} WHERE NAME = '{}' ", name));
    sql
}

pub async fn query_pdms_elements_type_name(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<String> {
    let sql = gen_query_pdms_elements_type_name_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    Ok(result.get::<String, _>("TYPE"))
}


/// 获得不同mdb下所有的world(因为numbdb有问题，暂时用这个替代)
// pub async fn query_mdb_module_worlds_fix(project_name: &str, pool: &Pool<MySql>, info_pool: &Pool<MySql>) -> anyhow::Result<HashMap<String, HashMap<String, Vec<RefU64>>>> {
//     let mut mdb_map = HashMap::new();
//     let mdbs = query_types_refnos(&vec!["MDB"], pool, None).await?;
//     for mdb in mdbs {
//         let mdb_attr = query_explicit_attr(mdb, pool).await?;
//         let mdb_name = query_name(mdb, &pool).await?;
//         let dbnos = query_project_dbno_info(project_name, info_pool).await?;
//         let mut map = HashMap::new();
//         for (db_type, dbnos) in dbnos {
//             for dbno in dbnos {
//                 if let Some(world_refno) = query_dbno_world(dbno, pool).await? {
//                     map.entry(db_type.to_string()).or_insert_with(Vec::new).push(world_refno);
//                 }
//             }
//         }
//         mdb_map.entry(mdb_name).or_insert(map);
//     }
//     Ok(mdb_map)
// }


#[derive(Debug, Default)]
pub struct DbQuickInfo {
    pub refno: RefU64,
    pub world_refno: RefU64,
    pub db_num: i32,
    pub db_type: String,
    pub project: String,
    pub order_number: i32,
}

pub type MdbQuickInfoMap = HashMap<String, HashMap<String, Vec<DbQuickInfo>>>;


pub async fn query_types_refnos(type_names: &[&str], pool: &Pool<MySql>, dbnos: Option<Vec<i32>>) -> anyhow::Result<RefU64Vec> {
    let mut r = vec![];
    let sql = gen_query_type_refnos_sql(type_names, dbnos);
    let result = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for val in result {
        let v = val.get::<i64, _>(0) as u64;
        r.push(RefU64(v));
    }
    Ok(RefU64Vec(r))
}

pub async fn query_name(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<String> {
    let sql = gen_query_name_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    Ok(result.get::<String, _>("NAME"))
}

/// todo dbno may exist in other database
pub async fn query_dbno(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Option<i32>> {
    let sql = gen_query_dbno_from_db_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    return match result {
        Ok(v) => { Ok(Some(v.get::<i32, _>(0))) }
        Err(_) => { Ok(None) }
    };
}

/// 根据dbno，获取world
pub async fn query_world_refno_by_dbno(dbno: i32, pool: &Pool<MySql>) -> anyhow::Result<Option<RefU64>> {
    let sql = gen_query_id_by_dbno_type_sql(dbno, "WORL");
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    return match result {
        Ok(v) => { Ok(Some(RefU64(v.get::<i64, _>("ID") as u64))) }
        Err(e) => {
            info!("query_world_refno_by_dbno error : {}", sql);
            Ok(None)
        }
    };
}

/// 根据 dbno 和 type 查询 refno 和 name
pub async fn query_id_name_from_dbno_type(dbno: i32, type_name: &str, pool: &Pool<MySql>) -> anyhow::Result<Option<Vec<(RefU64, String)>>> {
    let sql = gen_query_id_name_from_dbno_type_sql(dbno, type_name);
    let result = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await;
    return match result {
        Ok(vals) => {
            let mut r = vec![];
            for v in vals {
                let refno = RefU64(v.get::<i64, _>("ID") as u64);
                let name = v.get::<String, _>("NAME");
                r.push((refno, name))
            }
            Ok(Some(r))
        }
        Err(_) => { Ok(None) }
    };
}

/// 根据 dbno 查询 refno name 和 type
pub async fn query_id_from_dbno_type(dbno: u32, pool: &Pool<MySql>) -> anyhow::Result<Option<Vec<(RefU64, String, String)>>> {
    let sql = gen_query_id_name_type_from_dbno(dbno);
    let result = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await;
    return match result {
        Ok(vals) => {
            let mut r = vec![];
            for v in vals {
                let refno = RefU64(v.get::<i64, _>("ID") as u64);
                let name = v.get::<String, _>("NAME");
                let type_name = v.get::<String, _>("TYPE");
                r.push((refno, name, type_name))
            }
            Ok(Some(r))
        }
        Err(e) => {
            dbg!(e);
            dbg!(&sql);
            Ok(None)
        }
    };
}

/// 指定type查询所有type符合该值的节点
pub async fn query_types_refnos_names(types: &[&str], pool: &Pool<MySql>) -> anyhow::Result<Vec<(RefU64, String)>> {
    let mut r = vec![];
    let sql = gen_query_types_refnos_names(types);
    let result = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await;
    match result {
        Ok(vals) => {
            for v in vals {
                let refno = RefU64(v.get::<i64, _>("ID") as u64);
                let name = v.get::<String, _>("NAME");
                r.push((refno, name));
            }
        }
        Err(e) => {
            dbg!(e);
            dbg!(&sql);
        }
    };
    Ok(r)
}

pub async fn query_all_type_name_refnos(att_type: &str, pool: &Pool<MySql>) -> anyhow::Result<Vec<String>> {
    let mut name_vec = vec![];
    let sql = gen_query_all_type_name_refnos(att_type);
    let results = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await;
    match results {
        Ok(results) => {
            for result in results {
                let mut name = result.get::<String, _>("NAME");
                if name.starts_with("/") {
                    name = name[1..].to_string();
                }
                name_vec.push(name);
            }
        }
        Err(err) => {
            dbg!(&err);
        }
    }
    Ok(name_vec)
}

/// 获取zone属于哪个专业
pub async fn get_zone_divco(refno: RefU64, pool: &Pool<MySql>) -> String {
    if let Ok(attr) = query_explicit_attr(refno, pool).await {
        if let Some(val) = attr.map.get(&NounHash(ATT_DIVCO as u32)) {
            return val.string_value();
        }
    }
    "".to_string()
}

pub async fn query_project_dbno_info(project_name: &str, info_pool: &Pool<MySql>) -> anyhow::Result<HashMap<String, Vec<i32>>> {
    let mut map = HashMap::new();
    let sql = gen_query_dbno_info_by_project(project_name);
    let results = sqlx::query(&sql).fetch_all(&mut info_pool.acquire().await?).await;
    if let Ok(results) = results {
        for result in results {
            let numbdb = result.get::<i32, _>("NUMBDB");
            let db_type = result.get::<String, _>("DB_TYPE");
            map.entry(db_type).or_insert_with(Vec::new).push(numbdb);
        }
    }
    Ok(map)
}

///检查refno是否存在PDMS_ELEMENTS的表中
pub async fn check_exist_refno(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<bool> {
    let sql = format!("SELECT EXISTS(SELECT 1 FROM {PDMS_ELEMENTS_TABLE} WHERE ID = {})", refno.0);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    Ok(result.get::<bool, _>(0))
}

fn gen_query_dbno_info_by_project(project_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT NUMBDB , DB_TYPE FROM {PDMS_DBNO_INFOS_TABLE} WHERE PROJECT = '{}'", project_name));
    sql
}

fn gen_query_types_refnos_names(types: &[&str]) -> String {
    let mut sql = String::new();
    let mut types_sql = String::new();
    for att_type in types {
        types_sql.push_str(&format!("'{}',", att_type));
    }
    types_sql.remove(types_sql.len() - 1);

    sql.push_str(&format!("SELECT ID,NAME FROM {PDMS_ELEMENTS_TABLE} WHERE TYPE IN ({})", types_sql));
    sql
}

#[inline]
fn gen_query_id_by_dbno_type_sql(dbno: i32, type_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID FROM {PDMS_ELEMENTS_TABLE} WHERE TYPE = '{}' AND NUMBDB = {} AND IS_DEL = 0 ; ", type_name, dbno));
    sql
}

#[inline]
fn gen_query_node_id_from_refno_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT OWNER,NAME,TYPE,CHILDREN_COUNT FROM {PDMS_ELEMENTS_TABLE} WHERE ID = {}", refno.0));
    sql
}

#[inline]
fn gen_query_id_name_from_dbno_type_sql(dbno: i32, type_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID ,NAME FROM {PDMS_ELEMENTS_TABLE} WHERE TYPE = '{}' AND NUMBDB = {} AND IS_DEL = 0 ; ", type_name, dbno));
    sql
}

#[inline]
fn gen_query_id_name_type_from_dbno(dbno: u32) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID ,NAME, TYPE FROM {PDMS_ELEMENTS_TABLE} WHERE NUMBDB = {} AND IS_DEL = 0 ; ", dbno));
    sql
}

#[inline]
fn gen_query_dbno_from_db_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT NUMBDB FROM DB WHERE ID = {}", refno.0));
    sql
}

#[inline]
fn gen_pdms_elements_dbno_sql(dbno: u32, type_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID, OWNER ,NAME FROM {PDMS_ELEMENTS_TABLE} WHERE NUMBDB = {} AND TYPE = '{}' AND IS_DEL = 0 ;", dbno, type_name));
    sql
}

#[inline]
fn gen_pdms_elements_get_children_ele_node_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID,NAME,TYPE,OWNER,ORDER_NUM,CHILDREN_COUNT FROM {PDMS_ELEMENTS_TABLE} WHERE OWNER = {} AND IS_DEL = 0 ", refno.0));
    sql
}

#[inline]
fn gen_pdms_elements_get_all_world_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID,NAME,TYPE FROM {PDMS_ELEMENTS_TABLE} WHERE OWNER = '0/0' AND IS_DEL = 0 ;"));
    sql
}

#[inline]
fn gen_pdms_elements_get_children_count_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT COUNT(*) FROM {PDMS_ELEMENTS_TABLE} WHERE OWNER = {} AND IS_DEL = 0", refno.0));
    sql
}

#[inline]
pub fn gen_pdms_element_insert_sql(att: &WholeAttMap, name: &str, dbno: u32, order: usize, children_count: usize) -> String {
    let implicit = &att.implicit_attmap;
    let refno = implicit.get_refno().unwrap();
    let type_name = implicit.get_type();
    let owner = implicit.get_owner().unwrap();

    let mut sql = String::new();
    sql.push_str(&format!(r#"({}, '{}', '{}', {},'{}' , {} , {} , {} ,0 ) ,"#,
                          refno.0, refno.to_refno_str(), type_name, owner.0, name, dbno, order, children_count));
    sql
}

#[inline]
pub fn gen_dbinfo_value_insert_sql(dbno: u32, filename: &str, version: u32, project: &str, db_type: String) -> String {
    let mut sql = String::new();
    sql.push_str(&format!(r#"('{}', {},'{}',{} , '{}','{}')"#, format!("{}_{}", project, dbno), dbno, filename, version, project, db_type));
    sql
}

pub fn get_name(whole_attr: &DashMap<RefU64, WholeAttMap>, children_map: &HashMap<RefU64, RefU64Vec>, refno: RefU64) -> String {
    let attr = whole_attr.get(&refno).unwrap();
    let type_name = attr.implicit_attmap.get_type();
    return if let Some(name) = attr.explicit_attmap.get(&NounHash(db1_hash("NAME"))) {
        name.string_value()
    } else {
        let owner = attr.implicit_attmap.get_owner().unwrap();
        let mut idx = 1;
        if let Some(children) = children_map.get(&owner) {
            idx = children.iter().filter(|child| {
                if let Some(v) = whole_attr.get(child) {
                    whole_attr.get(child).unwrap().implicit_attmap.get_type() == type_name
                } else {
                    false
                }
            }).position(|node| node == &refno).unwrap_or_default() + 1;
        }
        format!("{} {}", type_name, idx)
    };
}

#[inline]
pub fn get_order(whole_attr: &DashMap<RefU64, WholeAttMap>, children_map: &HashMap<RefU64, RefU64Vec>, refno: RefU64) -> usize {
    let attr = whole_attr.get(&refno).unwrap();
    let owner = attr.implicit_attmap.get_owner().unwrap();
    if let Some(children) = children_map.get(&owner) {
        return children.iter().position(|child| child == &refno).unwrap_or_default();
    }
    0
}

#[inline]
pub fn gen_query_refno_type_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT TYPE FROM {PDMS_ELEMENTS_TABLE} WHERE ID = {} AND IS_DEL = 0 ", refno.0));
    sql
}

#[inline]
pub fn gen_query_type_refnos_sql(type_names: &[&str], dbnos: Option<Vec<i32>>) -> String {
    let mut sql = String::new();
    let mut in_sql = " (".to_string();
    for type_name in type_names {
        in_sql.push_str(&format!(r#"'{type_name}',"#));
    }
    in_sql.remove(in_sql.len() - 1);
    in_sql.push_str(") ");

    let mut dbnos_filter_sql = "".to_string();
    if dbnos.is_some() {
        let dbnos = dbnos.unwrap();
        if dbnos.len() > 0 {
            let sql_str = dbnos.iter().map(|x| x.to_string()).join(",");
            dbnos_filter_sql.push_str(&format!(" AND NUMBDB IN ({sql_str}) "));
        }
    }

    sql.push_str(&format!("SELECT ID FROM {PDMS_ELEMENTS_TABLE} WHERE TYPE IN {in_sql} {dbnos_filter_sql} AND IS_DEL = 0 ORDER BY ID;"));
    sql
}

#[inline]
pub fn gen_query_name_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT NAME FROM {PDMS_ELEMENTS_TABLE} WHERE ID = {} AND IS_DEL = 0;", refno.0));
    sql
}

#[inline]
fn gen_query_all_type_name_refnos(att_type: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT NAME FROM {PDMS_ELEMENTS_TABLE} WHERE TYPE = '{}' AND IS_DEL = 0 ", att_type));
    sql
}


#[tokio::test]
async fn test_get_mdb_type() -> anyhow::Result<()> {
    // let url = env::var("DATABASE_URL")?;
    // let info_pool = AiosDBManager::get_db_pool(&url, "pdms_info_db").await?;
    // let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    // let project = query_mdb_module_world_refnos(&pool, &info_pool, ).await?;
    // if let Some(v) = project.get("/SAMPLE") {
    //     if let Some(val) = v.get("DESI") {
    //         println!("val={:?}", val);
    //     }
    // }
    // println!("v={:?}", project);
    Ok(())
}

#[tokio::test]
async fn test_query_world() -> anyhow::Result<()> {
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let v = query_world("SAMPLE", "DESI", &pool).await?;
    println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_query_world_children() -> anyhow::Result<()> {
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let v = query_world_children("SAMPLE", "DESI", &pool).await?;
    println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_query_children_pdms_tree() -> anyhow::Result<()> {
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let refno: RefU64 = RefI32Tuple((15392, 0)).into();
    let v = query_children_pdms_tree("SAMPLE", "DESI", refno, &pool).await?;
    println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_query_owner_from_id() -> anyhow::Result<()> {
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let refno: RefU64 = RefI32Tuple((0, 0)).into();
    let v = query_owner_from_id(refno, &pool).await?;
    println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_query_world_ele_node() -> anyhow::Result<()> {
    // let url = env::var("DATABASE_URL")?;
    // let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    // let v = query_world_ele_node("SAMPLE", "DESI", &pool).await?;
    // println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_query_children_ele_node() -> anyhow::Result<()> {
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let refno: RefU64 = RefI32Tuple((23584, 5)).into();
    let v = query_children_eles(refno, &pool).await?;
    println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_query_children_pdms_tree_ele_node() -> anyhow::Result<()> {
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let refno: RefU64 = RefI32Tuple((15392, 0)).into();
    // let v = query_children_pdms_tree_ele_node("SAMPLE", "DESI", refno, &pool).await?;
    // println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_get_zone_divco() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let refno: RefU64 = RefI32Tuple((2013286748, 51294)).into();
    let v = get_zone_divco(refno, &pool).await;
    println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_query_project_dbno_info() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "pdms_info_db_sample").await?;
    let result = query_project_dbno_info("Sample", &pool).await?;
    dbg!(&result);
    Ok(())
}

#[tokio::test]
async fn test_query_world_children_eles() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "avevamarinesample").await?;
    let result = query_world_children_eles("ALL", "DESI", &pool).await?;
    dbg!(&result);
    Ok(())
}