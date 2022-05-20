use aios_core::pdms_types::{AttrInfo, AttrMap, AttrVal, DbAttributeType, NounHash, RefU64};
use anyhow::anyhow;
use sqlx::{Error, MySql, Pool, pool, Row};
use parse_pdms_db::db_tool::db1_hash;
use parse_pdms_db::db1_dehash;
use smol_str::SmolStr;
use dashmap::DashMap;
use glam::{Quat, Vec3};
use sqlx::Executor;
use sqlx::mysql::MySqlRow;
use crate::api::element::{query_pdms_elements_type_name, query_refno_type};
use crate::REFNO_INFO_MAP;
use crate::consts::*;

pub async fn query_implicit_attr(refno: RefU64, pool: Pool<MySql>) -> anyhow::Result<AttrMap> {
    let mut r = AttrMap::default();
    let type_name = query_pdms_elements_type_name(refno, pool.clone()).await?;
    let type_hash = db1_hash(&type_name);
    let sql = gen_query_implicit_attr_sql(refno, &type_name);
    let query_r = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    if let Some(val) = REFNO_INFO_MAP.get(&(type_hash as i32)) {
        for info in val.value() {
            if info.offset != 0 {
                let default_type = db1_dehash(*info.key() as u32).to_lowercase();
                match info.att_type {
                    DbAttributeType::INTEGER => {
                        let v = query_r.get::<i32, _>(default_type.as_str());
                        r.entry(NounHash(*info.key() as u32)).or_insert(AttrVal::IntegerType(v));
                    }
                    DbAttributeType::DOUBLE => {
                        let v = query_r.get::<f64, _>(default_type.as_str());
                        r.entry(NounHash(*info.key() as u32)).or_insert(AttrVal::DoubleType(v));
                    }
                    DbAttributeType::BOOL => {
                        let v = query_r.get::<bool, _>(default_type.as_str());
                        r.entry(NounHash(*info.key() as u32)).or_insert(AttrVal::BoolType(v));
                    }
                    DbAttributeType::STRING => {
                        let v = SmolStr::new(query_r.get::<String, _>(default_type.as_str()));
                        r.entry(NounHash(*info.key() as u32)).or_insert(AttrVal::StringType(v));
                    }
                    DbAttributeType::ELEMENT => {
                        let v = query_r.get::<i64, _>(default_type.as_str());
                        r.entry(NounHash(*info.key() as u32)).or_insert(AttrVal::RefU64Type(RefU64(v as u64)));
                    }
                    DbAttributeType::WORD => {
                        let v = SmolStr::new(query_r.get::<String, _>(default_type.as_str()));
                        r.entry(NounHash(*info.key() as u32)).or_insert(AttrVal::StringType(v));
                    }
                    DbAttributeType::DOUBLEVEC => {
                        let v = query_r.get::<Vec<u8>, _>(default_type.as_str());
                        let v = bincode::deserialize::<Vec<f64>>(&v).unwrap();
                        r.entry(NounHash(*info.key() as u32)).or_insert(AttrVal::DoubleArrayType(v));
                    }
                    DbAttributeType::INTVEC => {
                        let v = query_r.get::<String, _>(default_type.as_str());
                        let v = serde_json::from_str::<Vec<i32>>(&v).unwrap();
                        r.entry(NounHash(*info.key() as u32)).or_insert(AttrVal::IntArrayType(v));
                    }
                    DbAttributeType::Vec3Type => {
                        let v = query_r.get::<String, _>(default_type.as_str());
                        let v = serde_json::from_str::<[f64; 3]>(&v).unwrap();
                        r.entry(NounHash(*info.key() as u32)).or_insert(AttrVal::Vec3Type(v));
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(r)
}

pub async fn query_explicit_attr(refno: RefU64, pool: Pool<MySql>) -> anyhow::Result<AttrMap> {
    let sql = gen_query_explicit_attr_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    let val = result.get::<Vec<u8>, _>("data");
    Ok(bincode::deserialize::<AttrMap>(&val)?)
}

pub async fn query_full_attr(refno: RefU64, pool: Pool<MySql>) -> anyhow::Result<AttrMap> {
    let mut implicit_attr = query_implicit_attr(refno, pool.clone()).await?;
    let explicit_attr = query_explicit_attr(refno, pool.clone()).await?;
    for (k, v) in explicit_attr.map {
        implicit_attr.entry(k).or_insert(v);
    }
    Ok(implicit_attr)
}

pub async fn insert_attr_info(pool: Pool<MySql>) -> anyhow::Result<()> {
    let sql = gen_insert_attr_info_sql(&REFNO_INFO_MAP);
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

pub async fn query_position_from_id(refno: RefU64, pool: Pool<MySql>) -> anyhow::Result<Option<Vec3>> {
    let type_name = query_refno_type(refno, pool.clone()).await?;
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

pub async fn query_ori_from_id(refno: RefU64, pool: Pool<MySql>) -> anyhow::Result<Option<Quat>> {
    let type_name = query_refno_type(refno, pool.clone()).await?;
    let sql = gen_query_ori_from_id(refno, &type_name);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    return match result {
        Ok(result) => {
            let ang: [f64; 3] = serde_json::from_str(&result.get::<String, _>(0)).unwrap();
            let mat = (glam::f32::Mat3::from_rotation_z(ang[2].to_radians() as f32)
                * glam::f32::Mat3::from_rotation_y(ang[1].to_radians() as f32)
                * glam::f32::Mat3::from_rotation_x(ang[0].to_radians() as f32));
            Ok(Some(Quat::from_mat3(&mat)))
        }
        Err(_) => { Ok(None) }
    };
}

fn gen_insert_attr_info_sql(attr_info: &DashMap<i32, DashMap<i32, AttrInfo>>) -> String {
    let mut sql = String::new();
    sql.push_str("insert ignore into attr_info (type_hash, type,info ) Values ");
    for info in attr_info {
        let type_hash = *info.key() as u32;
        let type_name = db1_dehash(type_hash);
        let info = hex::encode(bincode::serialize(&info.value()).unwrap());
        sql.push_str(&format!("( {} , '{}', 0x{} ),", type_hash, type_name, info));
    }
    sql.remove(sql.len() - 1);
    sql
}

pub fn gen_query_implicit_attr_sql(refno: RefU64, type_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select * from {} where id = {}", type_name, refno.0));
    sql
}

pub fn gen_query_explicit_attr_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select * from {PDMS_EXPLICIT_TABLE} where id = {} ;", refno.0));
    sql
}

fn gen_query_ori_from_id(refno: RefU64, type_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select ori from {} where id = {} ;", type_name, refno.0));
    sql
}

fn gen_position_from_id(refno: RefU64, type_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select pos from {} where id = {} ;", refno.0, type_name));
    sql
}