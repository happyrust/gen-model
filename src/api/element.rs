use aios_core::pdms_types::{AiosStr, NounHash, RefU64, RefU64Vec};
use sqlx::{MySql, Pool, Row};
use std::collections::{BTreeMap, HashMap};
use smol_str::SmolStr;
use parse_pdms_db::parse::WholeAttMap;
use dashmap::DashMap;
use parse_pdms_db::db_tool::db1_hash;
use crate::sql::gen_sql::{gen_query_refno_type_sql, gen_query_type_refnos_sql};
use crate::sql::query_sql;

pub async fn query_refno_type(refno:RefU64, pool:Pool<MySql>) -> anyhow::Result<String> {
    let sql = gen_query_refno_type_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    Ok(result.get::<String,_>(0))
}

pub async fn query_children_pdms_tree(refno:RefU64, pool:Pool<MySql>) -> anyhow::Result<Vec<(RefU64, AiosStr)>> {
    let type_name = query_refno_type(refno,pool.clone()).await?;
    return if type_name == "WORL" {
        query_world_children(pool.clone()).await
    } else {
        query_children(refno, pool.clone()).await
    }
}

pub async fn query_world_children(pool:Pool<MySql>) -> anyhow::Result<Vec<(RefU64, AiosStr)>> {
    let mut b_map = BTreeMap::new();
    let sql = gen_query_type_refnos_sql("WORL");
    // 找到所有的world
    let result = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    let mut v = vec![];
    for r in result {
        let refno = r.get::<i64,_>(0);
        // 找到所有的world 对应的children
        let children = query_children(RefU64(refno as u64),pool.clone()).await?;
        b_map.insert(refno,children);
    }
    for (_,val) in b_map  {
        v.push(val);
    }
    Ok(v.into_iter().flatten().collect::<Vec<(RefU64,AiosStr)>>())
}

/// 获取某个refno 的 children 并未合并 world
pub async fn query_children(refno: RefU64, pool: Pool<MySql>) -> anyhow::Result<Vec<(RefU64,AiosStr)>> {
    let mut r = vec![];
    let mut b_map = BTreeMap::new();
    let sql = gen_pdms_elements_get_children_sql(refno);
    let vals = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    for val in vals {
        let child_refno = RefU64(val.get::<i64, _>("id") as u64);
        let name = AiosStr(SmolStr::new(val.get::<String, _>("name")));
        let order = val.get::<i32,_>("order_num");
        b_map.insert(order,(child_refno,name));
    }
    for (_,v) in b_map {
        r.push(v);
    }
    Ok(r)
}

pub async fn query_children_count(refno: RefU64, pool: Pool<MySql>) -> anyhow::Result<usize> {
    let count_sql = gen_pdms_elements_get_children_count_sql(refno);
    let count_result = sqlx::query(&count_sql).fetch_one(&mut pool.acquire().await?).await?;
    Ok(count_result.get::<i32, _>(0) as usize)
}

pub async fn query_world(main_db: u32, pool: Pool<MySql>) -> anyhow::Result<(RefU64, AiosStr)> {
    let sql = gen_pdms_elements_dbno_sql(main_db, "WORL");
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    let refno = RefU64(result.get::<i64, _>("id") as u64);
    let name = AiosStr(SmolStr::new(result.get::<String,_>("name")));
    Ok((refno,name))
}

fn gen_query_refno_infos_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select project from refno_infos where ref0 = {} limit 1;", refno.get_0()));
    sql
}

pub async fn query_refno_infos(refno: RefU64, pool: Pool<MySql>) -> anyhow::Result<String> {
    let sql = gen_query_refno_infos_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    let val = result.get::<String, _>("project");
    Ok(val)
}

fn gen_query_pdms_elements_type_name_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select type from pdms_elements where id = {}", refno.0));
    sql
}

pub async fn query_pdms_elements_type_name(refno: RefU64, pool: Pool<MySql>) -> anyhow::Result<String> {
    let sql = gen_query_pdms_elements_type_name_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    Ok(result.get::<String, _>("type"))
}

fn gen_query_implicit_attr_sql(refno: RefU64, type_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select * from {} where id = {}", type_name, refno.0));
    sql
}

fn gen_query_world_sql(pool: Pool<MySql>) -> String {
    let mut sql = String::new();
    sql.push_str("select * from worl");
    sql
}

fn gen_pdms_elements_dbno_sql(dbno: u32, type_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select id, owner ,name from pdms_elements where dbno = {} and type = '{}' ;", dbno, type_name));
    sql
}

fn gen_pdms_elements_get_children_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select id,name,type,order_num from pdms_elements where owner = {}", refno.0));
    sql
}

fn gen_pdms_elements_get_all_world_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select id,name,type from pdms_elements where owner = '0/0' ;"));
    sql
}

fn gen_pdms_elements_get_children_count_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select count(*) from pdms_elements where owner = {}", refno.0));
    sql
}

pub fn gen_pdms_element_insert_sql(att: &WholeAttMap, name:&str, dbno: u32, order: usize) -> String {
    let implicit = &att.implicit_attmap;
    let refno = implicit.get_refno().unwrap();
    let type_name = implicit.get_type();
    let owner = implicit.get_owner().unwrap();

    let mut sql = String::new();
    sql.push_str(&format!(r#"({}, '{}', '{}', {},'{}' , {} , {} ) ,"#,
                          refno.0, refno.to_refno_str(), type_name, owner.0, name, dbno,order));
    sql
}

pub fn gen_refno_infos_insert_sql(refno: RefU64, project: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!(r#"({},'{}') ,"#, refno.get_0(), project));
    sql
}

pub fn gen_dbno_filename_insert_sql(dbno: u32, filename: &str, version: u32, project:&str, db_type:SmolStr) -> String {
    let mut sql = String::new();
    sql.push_str(&format!(r#"({},'{}',{} , '{}','{}' ) ,"#, dbno, filename, version,project,db_type));
    sql
}

pub fn get_name(whole_attr: &DashMap<RefU64, WholeAttMap>, children_map: &HashMap<RefU64, RefU64Vec>, refno: RefU64) -> String {
    let attr = whole_attr.get(&refno).unwrap();
    let type_name = attr.implicit_attmap.get_type();
    return if let Some(name) = attr.explicit_attmap.get(&NounHash(db1_hash("NAME"))) {
        name.string_value().to_string()
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
    }
}

pub fn get_order(whole_attr: &DashMap<RefU64, WholeAttMap>, children_map:&HashMap<RefU64,RefU64Vec>, refno:RefU64) -> usize {
    let attr = whole_attr.get(&refno).unwrap();
    let owner = attr.implicit_attmap.get_owner().unwrap();
    if let Some(children) = children_map.get(&owner) {
        return children.iter().position(|child| child == &refno).unwrap_or_default();
    }
    0
}
