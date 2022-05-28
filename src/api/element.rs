use aios_core::pdms_types::*;
use sqlx::{Error, MySql, Pool, Row};
use std::collections::{BTreeMap, HashMap};
use std::env;
use aios_core::tool::db_tool::db1_hash;
use anyhow::anyhow;
use smol_str::SmolStr;
use parse_pdms_db::parse::WholeAttMap;
use dashmap::DashMap;
use futures::poll;
use sea_orm::sea_query::any;
use sqlx::mysql::{MySqlQueryResult, MySqlRow};
use crate::api::attr::{query_explicit_attr, query_implicit_attr};
use crate::api::dbno_filename::{query_dbtype_from_dbno, query_dbtype_from_dbno_count};
use crate::api::project_mdb::query_world_data;
use crate::api::test_sample::{get_test_info_pool, get_test_sample_pool};
use crate::consts::*;
use crate::data_interface::tidb_manager::AiosDBManager;


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

pub async fn query_children_pdms_tree_ele_node(mdb: &str, model: &str, refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Vec<PdmsElement>> {
    let type_name = query_refno_type(refno, &pool).await?;
    return if type_name == "WORL" {
        query_world_children_eles(mdb, model, &pool).await
    } else {
        query_children_eles(refno, &pool).await
    };
}

pub async fn query_world_children(mdb: &str, model: &str, pool: &Pool<MySql>) -> anyhow::Result<Vec<(RefU64, String)>> {
    let mut result = vec![];
    let mdb = format!("/{}", mdb);
    let world_data = query_world_data(&mdb, model, pool).await?;
    let data: Vec<RefU64> = bincode::deserialize(&world_data).unwrap();
    for world in data {
        let children = query_children(world, pool).await?;
        result.push(children);
    }
    Ok(result.into_iter().flatten().collect())
}

/// 获取world下的pdms elements
pub async fn query_world_children_eles(mdb: &str, model: &str, pool: &Pool<MySql>) -> anyhow::Result<Vec<PdmsElement>> {
    let mut result = vec![];
    let mdb = format!("/{mdb}");
    let world_data = query_world_data(&mdb, model, pool).await?;
    let data: Vec<RefU64> = bincode::deserialize(&world_data).unwrap();
    for world in data {
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
        let child_refno = RefU64(val.get::<i64, _>("id") as u64);
        let name = val.get::<String, _>("name");
        let order = val.get::<i32, _>("order_num");
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
        let child_refno = RefU64(val.get::<i64, _>("id") as u64);
        let name = val.get::<String, _>("name");
        let type_name = val.get::<String, _>("type");
        let owner = RefU64(val.get::<i64, _>("owner") as u64);
        let order = val.get::<i32, _>("order_num");
        let children_count = query_children_count(child_refno, &pool).await?;
        b_map.insert(order, PdmsElement {
            refno: child_refno.to_refno_string(),
            owner,
            name,
            noun: type_name,
            version: 0,
            children_count,
        });
    }
    for (_, v) in b_map {
        r.push(v);
    }
    Ok(r)
}

pub async fn query_children_count(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<usize> {
    let count_sql = gen_pdms_elements_get_children_count_sql(refno);
    let count_result = sqlx::query(&count_sql).fetch_one(&mut pool.acquire().await?).await?;
    Ok(count_result.get::<i32, _>(0) as usize)
}

pub async fn query_world(mdb: &str, module: &str, pool: &Pool<MySql>) -> anyhow::Result<EleTreeNode> {
    let mdb = format!("/{}", mdb);
    let world_data = query_world_data(&mdb, module, pool).await?;
    let data: Vec<RefU64> = bincode::deserialize(&world_data).unwrap();
    let refno = data[0];
    query_ele_node(refno, pool).await
}

/// 查询生成Element node
pub async fn query_ele_node(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<EleTreeNode> {
    let sql = format!("select * from {PDMS_ELEMENTS_TABLE} where id = {} and is_del = 0;", *refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    Ok(EleTreeNode{
        refno,
        noun: result.get::<String, _>("type"),
        name: result.get::<String, _>("name"),
        owner: RefU64::from(result.get::<i64, _>("owner") as u64),
    })
}

pub async fn query_world_ele_node(mdb: &str, module: &str, pool: &Pool<MySql>) -> anyhow::Result<Option<PdmsElement>> {
    let mdb = format!("/{}",mdb);
    let world_data = query_world_data(&mdb, module, &pool).await?;
    let data: Vec<RefU64> = bincode::deserialize(&world_data).unwrap();
    let world_refno = data[0];
    let sql = gen_query_node_id_from_refno_sql(world_refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    return match result {
        Ok(val) => {
            let owner = RefU64(val.get::<i64, _>("owner") as u64);
            let name = val.get::<String, _>("name");
            let type_name = val.get::<String, _>("type");
            let children_count = query_children_count(world_refno, pool).await?;
            Ok(Some(PdmsElement {
                refno: world_refno.to_refno_string(),
                owner,
                name,
                noun: type_name,
                version: 0,
                children_count,
            }))
        }
        Err(e) => {
            dbg!(e);
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

/// 生产
fn gen_query_refno_infos_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select project from {PDMS_REFNO_INFOS_TABLE} where ref0 = {} limit 1;", refno.get_0()));
    sql
}

// pub async fn query_project_hash(refno: RefU64, pool: Pool<MySql>) -> anyhow::Result<u32> {
//     let sql = gen_query_refno_infos_sql(refno);
//     let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
//     let val = result.get::<String, _>("project");
//     Ok(val)).get_u32_hash())
// }
/// 通过 name 获取 refno （pdms）
pub async fn query_id_from_name(name: &str, pool: Pool<MySql>) -> anyhow::Result<Option<RefU64>> {
    let sql = gen_query_id_from_name_sql(name);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    match result {
        Ok(v) => { Ok(Some(RefU64(v.get::<i64, _>(0) as u64))) }
        Err(_) => { Ok(None) }
    }
}

/// 通过 name 获取 refno （ssc）
pub async fn query_id_from_name_ssc(name: &str, pool: Pool<MySql>) -> anyhow::Result<Option<RefU64>> {
    let sql = gen_query_id_from_name_ssc_sql(name);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    match result {
        Ok(v) => {
            let refno = RefU64(v.get::<i64, _>("id") as u64);
            Ok(Some(refno))
        }
        Err(_) => { Ok(None) }
    }
}

fn gen_query_pdms_elements_type_name_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT type FROM {PDMS_ELEMENTS_TABLE} WHERE id = {} AND is_del = 0 ", refno.0));
    sql
}

fn gen_query_owner_from_id(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select owner from {PDMS_ELEMENTS_TABLE} where id = {} and is_del = 0 ", refno.0));
    sql
}

fn gen_query_id_from_name_sql(name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select id from {PDMS_ELEMENTS_TABLE} where name = '{}' ", name));
    sql
}

fn gen_query_id_from_name_ssc_sql(name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select id from {PDMS_SSC_ELEMENTS_TABLE} where name = '{}' ", name));
    sql
}

pub async fn query_pdms_elements_type_name(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<String> {
    let sql = gen_query_pdms_elements_type_name_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    Ok(result.get::<String, _>("type"))
}

pub async fn query_mdb_module_worlds(pool: &Pool<MySql>, info_pool: &Pool<MySql>) -> anyhow::Result<HashMap<String, HashMap<String, Vec<RefU64>>>> {
    let mut result = HashMap::new();
    let mdbs = query_types_refnos(&vec!["MDB"], pool).await?;
    for mdb in mdbs {
        let mdb_attr = query_explicit_attr(mdb, pool).await?;
        let mdb_name = query_name(mdb, &pool).await?;
        if let Some(dbs) = mdb_attr.get(&NounHash(db1_hash("CURD"))) {
            let mut val = HashMap::new();
            let dbs = dbs.refu64_vec_value().unwrap();
            for db in dbs {
                if let Some(dbno) = query_dbno_from_db(db, pool).await? {
                    if let Some(db_type) = query_dbtype_from_dbno(dbno, info_pool).await? {
                        if let Some(world_refno) = query_dbno_world(dbno, pool).await? {
                            val.entry(db_type).or_insert_with(Vec::new).push(world_refno);
                        }
                    }
                }
            }
            result.entry(mdb_name).or_insert(val);
        }
    }
    Ok(result)
}

pub async fn query_types_refnos(type_names: &Vec<&str>, pool: &Pool<MySql>) -> anyhow::Result<RefU64Vec> {
    let mut r = vec![];
    let sql = gen_query_type_refnos_sql(type_names);
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
    Ok(result.get::<String, _>("name"))
}

pub async fn query_dbno_from_db(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Option<i32>> {
    let sql = gen_query_dbno_from_db_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    return match result {
        Ok(v) => { Ok(Some(v.get::<i32, _>(0))) }
        Err(_) => { Ok(None) }
    };
}

pub async fn query_dbno_world(dbno: i32, pool: &Pool<MySql>) -> anyhow::Result<Option<RefU64>> {
    let sql = gen_query_id_from_dbno_type_sql(dbno, "WORL");
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    return match result {
        Ok(v) => { Ok(Some(RefU64(v.get::<i64, _>(0) as u64))) }
        Err(_) => { Ok(None) }
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
                let refno = RefU64(v.get::<i64, _>("id") as u64);
                let name = v.get::<String, _>("name");
                r.push((refno, name))
            }
            Ok(Some(r))
        }
        Err(_) => { Ok(None) }
    };
}

/// 根据 dbno 查询 refno name 和 type
pub async fn query_id_from_dbno_type(dbno: u32, pool: &Pool<MySql>) -> anyhow::Result<Option<Vec<(RefU64, String,String)>>> {
    let sql = gen_query_id_name_type_from_dbno(dbno);
    let result = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await;
    return match result {
        Ok(vals) => {
            let mut r = vec![];
            for v in vals {
                let refno = RefU64(v.get::<i64, _>("id") as u64);
                let name = v.get::<String, _>("name");
                let type_name = v.get::<String,_>("type");
                r.push((refno, name,type_name))
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

fn gen_query_id_from_dbno_type_sql(dbno: i32, type_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select id from {PDMS_ELEMENTS_TABLE} where type = '{}' and dbno = {} and is_del = 0 ; ", type_name, dbno));
    sql
}

fn gen_query_node_id_from_refno_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select owner,name,type from {PDMS_ELEMENTS_TABLE} where id = {}", refno.0));
    sql
}

fn gen_query_id_name_from_dbno_type_sql(dbno: i32, type_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select id ,name from {PDMS_ELEMENTS_TABLE} where type = '{}' and dbno = {} and is_del = 0 ; ", type_name, dbno));
    sql
}

fn gen_query_id_name_type_from_dbno(dbno: u32) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select id ,name, type from {PDMS_ELEMENTS_TABLE} where dbno = {} and is_del = 0 ; ", dbno));
    sql
}

fn gen_query_dbno_from_db_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select dbno from db where id = {}", refno.0));
    sql
}

fn gen_pdms_elements_dbno_sql(dbno: u32, type_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select id, owner ,name from {PDMS_ELEMENTS_TABLE} where dbno = {} and type = '{}' and is_del = 0 ;", dbno, type_name));
    sql
}

fn gen_pdms_elements_get_children_ele_node_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select id,name,type,owner,order_num from {PDMS_ELEMENTS_TABLE} where owner = {} and is_del = 0 ", refno.0));
    sql
}

fn gen_pdms_elements_get_all_world_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select id,name,type from {PDMS_ELEMENTS_TABLE} where owner = '0/0' and is_del = 0 ;"));
    sql
}

fn gen_pdms_elements_get_children_count_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select count(*) from {PDMS_ELEMENTS_TABLE} where owner = {} and is_del = 0", refno.0));
    sql
}

pub fn gen_pdms_element_insert_sql(att: &WholeAttMap, name: &str, dbno: u32, order: usize) -> String {
    let implicit = &att.implicit_attmap;
    let refno = implicit.get_refno().unwrap();
    let type_name = implicit.get_type();
    let owner = implicit.get_owner().unwrap();

    let mut sql = String::new();
    sql.push_str(&format!(r#"({}, '{}', '{}', {},'{}' , {} , {} , 0 ) ,"#,
                          refno.0, refno.to_refno_str(), type_name, owner.0, name, dbno, order));
    sql
}

pub fn gen_dbno_filename_insert_sql(dbno: u32, filename: &str, version: u32, project: &str, db_type: SmolStr) -> String {
    let mut sql = String::new();
    sql.push_str(&format!(r#"({},'{}',{} , '{}','{}') ,"#, dbno, filename, version, project, db_type));
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

pub fn get_order(whole_attr: &DashMap<RefU64, WholeAttMap>, children_map: &HashMap<RefU64, RefU64Vec>, refno: RefU64) -> usize {
    let attr = whole_attr.get(&refno).unwrap();
    let owner = attr.implicit_attmap.get_owner().unwrap();
    if let Some(children) = children_map.get(&owner) {
        return children.iter().position(|child| child == &refno).unwrap_or_default();
    }
    0
}

pub fn gen_query_refno_type_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select type from {PDMS_ELEMENTS_TABLE} where id = {} and is_del = 0 ", refno.0));
    sql
}

pub fn gen_query_type_refnos_sql(type_names: &Vec<&str>) -> String {
    let mut sql = String::new();
    let mut in_sql = " (".to_string();
    for type_name in type_names {
        in_sql.push_str(&format!(r#"'{type_name}',"#));
    }
    in_sql.remove(in_sql.len() - 1);
    in_sql.push_str(") ");
    sql.push_str(&format!("select id from {PDMS_ELEMENTS_TABLE} where type in {in_sql} and is_del = 0 order by id;"));
    sql
}

pub fn gen_query_name_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select name from {PDMS_ELEMENTS_TABLE} where id = {} and is_del = 0;", refno.0));
    sql
}

#[tokio::test]
async fn test_get_mdb_type() -> anyhow::Result<()> {
    let url = env::var("DATABASE_URL")?;
    let info_pool = AiosDBManager::get_db_pool(&url,"pdms_info_db").await?;
    let pool = AiosDBManager::get_db_pool(&url,"sample").await?;
    let project = query_mdb_module_worlds(&pool, &info_pool).await?;
    if let Some(v) = project.get("/SAMPLE") {
        if let Some(val) = v.get("DESI") {
            println!("val={:?}", val);
        }
    }
    println!("v={:?}", project);
    Ok(())
}

#[tokio::test]
async fn test_query_world() -> anyhow::Result<()> {
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url,"sample").await?;
    let v = query_world("SAMPLE", "DESI", &pool).await?;
    println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_query_world_children() -> anyhow::Result<()> {
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url,"sample").await?;
    let v = query_world_children("SAMPLE", "DESI", &pool).await?;
    println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_query_children_pdms_tree() -> anyhow::Result<()> {
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url,"sample").await?;
    let refno: RefU64 = RefI32Tuple((15392, 0)).into();
    let v = query_children_pdms_tree("SAMPLE", "DESI", refno, &pool).await?;
    println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_query_owner_from_id() -> anyhow::Result<()> {
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url,"sample").await?;
    let refno: RefU64 = RefI32Tuple((0, 0)).into();
    let v = query_owner_from_id(refno, &pool).await?;
    println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_query_world_ele_node() -> anyhow::Result<()> {
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url,"sample").await?;
    let v = query_world_ele_node("SAMPLE", "DESI", &pool).await?;
    println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_query_children_ele_node() -> anyhow::Result<()> {
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url,"sample").await?;
    let refno: RefU64 = RefI32Tuple((23584, 5)).into();
    let v = query_children_eles(refno, &pool).await?;
    println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_query_children_pdms_tree_ele_node() -> anyhow::Result<()> {
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url,"sample").await?;
    let refno: RefU64 = RefI32Tuple((15392, 0)).into();
    let v = query_children_pdms_tree_ele_node("SAMPLE", "DESI", refno, &pool).await?;
    println!("v={:?}", v);
    Ok(())
}