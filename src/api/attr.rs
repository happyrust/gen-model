use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use aios_core::cache::refno::CachedRefBasic;
use aios_core::consts::*;
use aios_core::pdms_types::{AttrInfo, AttrMap, AttrVal, DbAttributeType, NounHash, RefI32Tuple, RefU64};
use aios_core::pdms_types::AttrVal::StringType;
use aios_core::tool::db_tool::{db1_dehash, db1_hash};
use anyhow::anyhow;
use sqlx::{Error, MySql, Pool, pool, Row};
use smol_str::SmolStr;
use dashmap::DashMap;
use glam::{Quat, Vec3};
use itertools::Itertools;
use sqlx::Executor;
use sqlx::mysql::MySqlRow;
use crate::api::element::{query_ele_node, query_owner_from_id, query_pdms_elements_type_name, query_refno_type, query_types_refnos};
use crate::ATTR_INFO_MAP;
use crate::consts::*;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::helper::qualified_table_name;

/// 指定从特定的表查询数据，根据owner查询
pub async fn query_implicit_attrs_by_owner(owner: RefU64, type_name: &str, pool: &Pool<MySql>, column_names: Option<Vec<&str>>) -> anyhow::Result<Vec<AttrMap>> {
    let sql = gen_query_implicit_attr_sql_by_owner(owner, &type_name, &column_names);
    let column_names = column_names.unwrap_or_default();
    let rows = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await?;
    let type_hash = db1_hash(type_name.to_uppercase().as_str());

    let mut att_maps = vec![];
    for r in &rows {
        let a = convert_row_to_attmap(r, type_hash as i32, &column_names)?;
        att_maps.push(a);
    }
    Ok(att_maps)
}

#[inline]
pub fn convert_row_to_attmap(row: &MySqlRow, type_hash: i32, column_names: &Vec<&str>) -> anyhow::Result<AttrMap> {
    let mut r = AttrMap::default();
    if let Some(val) = ATTR_INFO_MAP.get(&type_hash) {
        for info in val.value() {
            if !column_names.is_empty() && !column_names.contains(&info.name.as_str()) {
                continue;
            }
            //type 需要获取
            if info.offset != 0 || info.hash as u32 == *TYPE_HASH {
                let t = info.name.as_str();
                let hash = NounHash::from(db1_hash(&info.name));
                match info.att_type {
                    DbAttributeType::INTEGER => {
                        row.try_get::<i32, _>(t).map(|v| {
                            r.entry(hash).or_insert(AttrVal::IntegerType(v))
                        })?;
                    }
                    DbAttributeType::DOUBLE => {
                        row.try_get::<f64, _>(t).map(|v| {
                            r.entry(hash).or_insert(AttrVal::DoubleType(v))
                        })?;
                    }
                    DbAttributeType::BOOL => {
                        row.try_get::<bool, _>(t).map(|v| {
                            r.entry(hash).or_insert(AttrVal::BoolType(v))
                        })?;
                    }
                    DbAttributeType::STRING => {
                        row.try_get::<String, _>(t).map(|v| {
                            r.entry(hash).or_insert(AttrVal::StringType(v.into()))
                        })?;
                    }
                    DbAttributeType::ELEMENT => {
                        row.try_get::<i64, _>(t).map(|v| {
                            r.entry(hash).or_insert(AttrVal::RefU64Type(RefU64(v as u64)))
                        })?;
                    }
                    DbAttributeType::WORD => {
                        row.try_get::<String, _>(t).map(|v| {
                            r.entry(hash).or_insert(AttrVal::StringType(SmolStr::new(v)))
                        })?;
                    }
                    DbAttributeType::DOUBLEVEC => {
                        row.try_get::<Vec<u8>, _>(t).map(|v| {
                            let v = bincode::deserialize::<Vec<f64>>(&v).unwrap();
                            r.entry(hash).or_insert(AttrVal::DoubleArrayType(v))
                        })?;
                    }
                    DbAttributeType::INTVEC => {
                        row.try_get::<String, _>(t).map(|v| {
                            let v = serde_json::from_str::<Vec<i32>>(&v).unwrap();
                            r.entry(hash).or_insert(AttrVal::IntArrayType(v))
                        })?;
                    }
                    DbAttributeType::Vec3Type | DbAttributeType::ORIENTATION | DbAttributeType::POSITION | DbAttributeType::DIRECTION => {
                        row.try_get::<String, _>(t).map(|v| {
                            let v = serde_json::from_str::<[f64; 3]>(&v).unwrap_or_default();
                            r.entry(hash).or_insert(AttrVal::Vec3Type(v))
                        })?;
                    }
                    _ => {}
                }
            }
        }
    }
    if column_names.contains(&"TYPE") {
        row.try_get::<String, _>("TYPE").map(|v| {
            r.entry(TYPE_HASH).or_insert(AttrVal::StringType(v.into()))
        })?;
    }
    if column_names.contains(&"NAME") {
        row.try_get::<String, _>("NAME").map(|v| {
            r.entry(NAME_HASH).or_insert(AttrVal::StringType(v.into()))
        })?;
    }
    if column_names.contains(&"OWNER") {
        row.try_get::<i64, _>("OWNER").map(|v| {
            r.entry(OWNER_HASH).or_insert(AttrVal::RefU64Type(RefU64(v as u64)))
        })?;
    }
    Ok(r)
}

/// 获得隐式属性
pub async fn query_implicit_attr(refno: RefU64, ref_basic: &CachedRefBasic, pool: &Pool<MySql>, column_names: Option<Vec<&str>>) -> anyhow::Result<AttrMap> {
    let type_name = ref_basic.get_type();
    let type_hash = *ref_basic.get_noun_hash() as i32;
    let mut exclude_columns = vec![];
    //需要过滤一遍
    let column_names = if column_names.is_some() {
        let mut column_names = column_names.unwrap();
        if column_names.len() == 0 { return Ok(AttrMap::default()); }
        if let Some(names_map) = ATTR_INFO_MAP.get_names_of_type(type_name) {
            exclude_columns = column_names.drain_filter(|x| {
                !names_map.value().contains(*x)
            }).collect();
        }
        column_names
    } else {
        vec![]
    };
    let sql = gen_query_implicit_attr_sql(refno, ref_basic.get_table_name(), &column_names);
    let row = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    let mut r = convert_row_to_attmap(&row, type_hash, &column_names)?;
    //其他的插入
    if exclude_columns.len() > 0 {
        exclude_columns.iter().for_each(|x| {
            let hash = NounHash::from(db1_hash(*x));
            r.insert(hash, AttrVal::InvalidType);
        });
    }
    Ok(r)
}

/// 查找整张表的 外键 refno 返回自身 refno + foreign refno
pub async fn query_foreign_refnos_from_table(noun: &str, table_name: &str, pool: &Pool<MySql>) -> anyhow::Result<Vec<(RefU64, RefU64)>> {
    let mut r = vec![];
    let sql = gen_query_value_from_table(noun, table_name);
    let results = sqlx::query(&sql).fetch_all(&mut pool.acquire().await?).await;
    match results {
        Ok(results) => {
            for result in results {
                let refno = RefU64(result.get::<i64, _>("ID") as u64);
                let foreign = RefU64(result.get::<i64, _>(noun) as u64);
                r.push((refno, foreign));
            }
        }
        Err(err) => {
            dbg!(sql);
            dbg!(err);
        }
    }
    Ok(r)
}

pub async fn query_explicit_attr(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<AttrMap> {
    let sql = gen_query_explicit_attr_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    let val = result.get::<Vec<u8>, _>("DATA");
    Ok(AttrMap::from_compress_bytes(&val).unwrap_or_default())
}

pub async fn query_uda_attr(att_type: &str, pool: &Pool<MySql>) -> anyhow::Result<AttrMap> {
    let sql = gen_query_uda_attr_sql(att_type);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    if result.is_err() { return Ok(AttrMap::default()); }
    let result = result.unwrap();
    let val = result.get::<Vec<u8>, _>("DATA");
    Ok(AttrMap::from_compress_bytes(&val).unwrap_or_default())
}

pub async fn query_full_attr(refno: RefU64, aios_mgr: &AiosDBManager, column_names: Option<Vec<&str>>) -> anyhow::Result<AttrMap> {
    if let Some(project) = aios_mgr.ref0_map.get(&refno.get_0()) {
        let pool = aios_mgr.project_map.get(project.value());
        if pool.is_none() { return Ok(AttrMap::default()); }
        let pool = pool.unwrap();
        let ref_basic = aios_mgr.get_refno_basic(refno);
        if ref_basic.is_none() { return Ok(AttrMap::default()); }
        let ref_basic = ref_basic.unwrap();

        let mut attr = query_implicit_attr(refno, ref_basic.value(), pool.value(), column_names).await?;
        let att_type = attr.get_type().to_string();
        let explicit_attr = query_explicit_attr(refno, pool.value()).await?;
        let ele = query_ele_node(refno, pool.value()).await?;
        for (k, v) in explicit_attr.map {
            attr.entry(k).or_insert(v);
        }
        for pool in &aios_mgr.project_map {
            // uda 赋值需要加上元件库
            let uda_attr = query_uda_attr(&att_type, pool.value()).await?;
            for (k, v) in uda_attr.map {
                attr.entry(k).or_insert(v);
            }
        }
        // 赋默认值
        if let Some(map) = ATTR_INFO_MAP.map.get(&(db1_hash(&ele.noun) as i32)) {
            for values in map.value() {
                attr.entry(NounHash(*values.key() as u32)).or_insert(values.default_val.clone());
            }
        }
        attr.insert(REFNO_HASH, AttrVal::RefU64Type(ele.refno));
        attr.insert(NAME_HASH, AttrVal::StringType(ele.name.into()));
        attr.insert(OWNER_HASH, AttrVal::RefU64Type(ele.owner));
        return Ok(attr);
    }
    Ok(AttrMap::default())
}


pub async fn insert_attr_info(pool: Pool<MySql>) -> anyhow::Result<()> {
    let sql = gen_insert_attr_info_sql(&ATTR_INFO_MAP);
    let mut conn = pool.acquire().await?;
    let result = conn.execute(sql.as_str()).await;
    match result {
        Ok(_) => {}
        Err(e) => {
            dbg!(e);
            dbg!(sql.as_str());
        }
    }
    Ok(())
}

pub async fn query_position_from_id(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Option<Vec3>> {
    let type_name = query_refno_type(refno, pool).await?;
    let sql = gen_position_from_id(refno, &type_name);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    return match result {
        Ok(v) => {
            let pos: [f64; 3] = serde_json::from_str(&v.get::<String, _>(0)).unwrap();
            Ok(Some(Vec3::new(pos[0] as f32, pos[1] as f32, pos[2] as f32)))
        }
        Err(_) => { Ok(None) }
    };
}

pub async fn query_ori_from_id(refno: RefU64, table_name: &str, pool: &Pool<MySql>) -> anyhow::Result<Option<Quat>> {
    let sql = gen_query_ori_from_id(refno, table_name);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    return match result {
        Ok(result) => {
            let ang: [f64; 3] = serde_json::from_str(&result.get::<String, _>(0)).unwrap_or([0.0, 0.0, 0.0]);
            let mat = (glam::f32::Mat3::from_rotation_z(ang[2].to_radians() as f32)
                * glam::f32::Mat3::from_rotation_y(ang[1].to_radians() as f32)
                * glam::f32::Mat3::from_rotation_x(ang[0].to_radians() as f32));
            Ok(Some(Quat::from_mat3(&mat)))
        }
        Err(_) => { Ok(None) }
    };
}

pub async fn query_foreign_refno(refno: RefU64, foreign_type: &str, pool: &Pool<MySql>) -> anyhow::Result<Option<RefU64>> {
    let type_name = query_refno_type(refno, pool).await?;
    let sql = gen_query_foreign_refno_sql(refno, &type_name, foreign_type);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    return match result {
        Ok(v) => {
            return Ok(Some(RefU64(v.get::<i64, _>(0) as u64)));
        }
        Err(_) => { Ok(None) }
    };
}

fn gen_insert_attr_info_sql(attr_info: &DashMap<i32, DashMap<i32, AttrInfo>>) -> String {
    let mut sql = String::new();
    sql.push_str("INSERT IGNORE INTO ATTR_INFO (TYPE_HASH, TYPE,INFO ) VALUES ");
    for info in attr_info {
        let type_hash = *info.key() as u32;
        let type_name = db1_dehash(type_hash);
        let info = hex::encode(bincode::serialize(&info.value()).unwrap());
        sql.push_str(&format!("( {} , '{}', 0x{} ),", type_hash, type_name, info));
    }
    sql.remove(sql.len() - 1);
    sql
}

#[inline]
pub fn gen_query_implicit_attr_sql(refno: RefU64, table_name: &str, columns: &Vec<&str>) -> String {
    let mut sql = String::new();
    let cols_sql = if columns.len() == 0 {
        "*".to_string()
    } else {
        columns.join(",")
    };
    sql.push_str(&format!("SELECT {cols_sql} FROM {} WHERE ID = {}", table_name, refno.0));
    sql
}

/// 生成通过owner获取的sql语句
#[inline]
pub fn gen_query_implicit_attr_sql_by_owner(owner: RefU64, type_name: &str, columns: &Option<Vec<&str>>) -> String {
    let table_name = qualified_table_name(type_name);
    let mut sql = String::new();
    let cols_sql = columns.as_ref().map(|x| {
        x.join(",")
    }).unwrap_or("*".to_string());
    sql.push_str(&format!("SELECT {cols_sql} FROM {} WHERE OWNER = {}", table_name, owner.0));
    sql
}

pub fn gen_query_explicit_attr_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT DATA FROM {PDMS_EXPLICIT_TABLE} WHERE ID = {} ;", refno.0));
    sql
}

pub fn gen_query_uda_attr_sql(att_type: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT DATA FROM {PDMS_UDA_TABLE} WHERE TYPE = '{}' ;", att_type));
    sql
}

fn gen_query_foreign_refno_sql(refno: RefU64, type_name: &str, foreign_type: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT {} FROM {} WHERE ID = {} ;", foreign_type, type_name, refno.0));
    sql
}

fn gen_query_ori_from_id(refno: RefU64, type_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ORI FROM {} where ID = {} ;", type_name, refno.0));
    sql
}

fn gen_position_from_id(refno: RefU64, type_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT POS FROM {} where ID = {} ;", type_name, refno.0));
    sql
}

fn gen_query_value_from_table(noun: &str, table_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ID , {} FROM {}", noun, table_name));
    sql
}

#[tokio::test]
async fn test_query_foreign_refno() -> anyhow::Result<()> {
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let refno: RefU64 = RefI32Tuple((23584, 121)).into();
    let v = query_foreign_refno(refno, "catr", &pool).await?;
    println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_query_position_refno() -> anyhow::Result<()> {
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let refno: RefU64 = RefI32Tuple((23584, 11)).into();
    let v = query_position_from_id(refno, &pool).await?;
    println!("v={:?}", v);
    Ok(())
}
