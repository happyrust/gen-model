use std::env;
use aios_core::pdms_types::{AttrInfo, AttrMap, AttrVal, DbAttributeType, NounHash, RefI32Tuple, RefU64};
use aios_core::tool::db_tool::{db1_dehash, db1_hash};
use anyhow::anyhow;
use sqlx::{Error, MySql, Pool, pool, Row};
use smol_str::SmolStr;
use dashmap::DashMap;
use glam::{Quat, Vec3};
use sqlx::Executor;
use sqlx::mysql::MySqlRow;
use crate::api::element::{query_pdms_elements_type_name, query_refno_type, query_type_refnos};
use crate::REFNO_INFO_MAP;
use crate::consts::*;
use crate::data_interface::tidb_manager::AiosDBManager;


/// 获得隐式属性
pub async fn query_implicit_attr(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<AttrMap> {
    let mut r = AttrMap::default();
    let type_name = query_pdms_elements_type_name(refno, pool).await?;
    let type_hash = db1_hash(&type_name);
    let sql = gen_query_implicit_attr_sql(refno, &type_name);
    let query_r = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    if let Some(val) = REFNO_INFO_MAP.get(&(type_hash as i32)) {
        for info in val.value() {
            if info.offset != 0 {
                // let default_type = db1_dehash(*info.key() as u32).to_lowercase();
                let t = info.name.to_lowercase();
                let t = t.as_str();
                match info.att_type {
                    DbAttributeType::INTEGER => {
                        let v = query_r.try_get::<i32, _>(t);
                        match v {
                            Ok(v) => {
                                r.entry(NounHash(*info.key() as u32)).or_insert(AttrVal::IntegerType(v));
                            }
                            Err(_) => {}
                        }
                    }
                    DbAttributeType::DOUBLE => {
                        let v = query_r.try_get::<f64, _>(t);
                        match v {
                            Ok(v) => {
                                r.entry(NounHash(*info.key() as u32)).or_insert(AttrVal::DoubleType(v));
                            }
                            Err(_) => {}
                        }
                    }
                    DbAttributeType::BOOL => {
                        let v = query_r.try_get::<bool, _>(t);
                        match v {
                            Ok(v) => {
                                r.entry(NounHash(*info.key() as u32)).or_insert(AttrVal::BoolType(v));
                            }
                            Err(_) => {}
                        }
                    }
                    DbAttributeType::STRING => {
                        let v = query_r.try_get::<String, _>(t);
                        match v {
                            Ok(v) => {
                                r.entry(NounHash(*info.key() as u32)).or_insert(AttrVal::StringType(SmolStr::new(v)));
                            }
                            Err(_) => {}
                        }
                    }
                    DbAttributeType::ELEMENT => {
                        let v = query_r.try_get::<i64, _>(t);
                        match v {
                            Ok(v) => {
                                r.entry(NounHash(*info.key() as u32)).or_insert(AttrVal::RefU64Type(RefU64(v as u64)));
                            }
                            Err(_) => {}
                        }
                    }
                    DbAttributeType::WORD => {
                        let v = query_r.try_get::<String, _>(t);
                        match v {
                            Ok(v) => {
                                r.entry(NounHash(*info.key() as u32)).or_insert(AttrVal::StringType(SmolStr::new(v)));
                            }
                            Err(_) => {}
                        }
                    }
                    DbAttributeType::DOUBLEVEC => {
                        let v = query_r.try_get::<Vec<u8>, _>(t);
                        match v {
                            Ok(v) => {
                                let v = bincode::deserialize::<Vec<f64>>(&v).unwrap();
                                r.entry(NounHash(*info.key() as u32)).or_insert(AttrVal::DoubleArrayType(v));
                            }
                            Err(_) => {}
                        }
                    }
                    DbAttributeType::INTVEC => {
                        let v = query_r.try_get::<String, _>(t);
                        match v {
                            Ok(v) => {
                                let v = serde_json::from_str::<Vec<i32>>(&v).unwrap();
                                r.entry(NounHash(*info.key() as u32)).or_insert(AttrVal::IntArrayType(v));
                            }
                            Err(_) => {}
                        }
                    }
                    DbAttributeType::Vec3Type => {
                        let v = query_r.try_get::<String, _>(t);
                        match v {
                            Ok(v) => {
                                let v = serde_json::from_str::<[f64; 3]>(&v).unwrap();
                                r.entry(NounHash(*info.key() as u32)).or_insert(AttrVal::Vec3Type(v));
                            }
                            Err(_) => {}
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(r)
}

pub async fn query_explicit_attr(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<AttrMap> {
    let sql = gen_query_explicit_attr_sql(refno);
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await?;
    let val = result.get::<Vec<u8>,_>("data");
    // Ok(bincode::deserialize::<AttrMap>(&val)?)
    Ok(AttrMap::from_compress_bytes(&val).unwrap_or_default())
}

pub async fn query_full_attr(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<AttrMap> {
    let mut implicit_attr = query_implicit_attr(refno, pool).await?;
    let explicit_attr = query_explicit_attr(refno, pool).await?;
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

pub async fn query_ori_from_id(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Option<Quat>> {
    let type_name = query_refno_type(refno, pool).await?;
    let sql = gen_query_ori_from_id(refno, &type_name);
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

fn gen_query_foreign_refno_sql(refno: RefU64, type_name: &str, foreign_type: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select {} from {} where id = {} ;", foreign_type, type_name, refno.0));
    sql
}

fn gen_query_ori_from_id(refno: RefU64, type_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select ori from {} where id = {} ;", type_name, refno.0));
    sql
}

fn gen_position_from_id(refno: RefU64, type_name: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("select pos from {} where id = {} ;", type_name, refno.0));
    sql
}

fn gen_test_not_exist_table_sql() -> String {
    let sql = "select * from acrw".to_string();
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

#[tokio::test]
async fn test_test_not_exist_table_sql() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "sample").await?;
    let sql = gen_test_not_exist_table_sql();
    let result = sqlx::query(&sql).fetch_one(&mut pool.acquire().await?).await;
    match result {
        Ok(v) => {
            let r = v.try_get::<String, _>("pos");
            match r {
                Ok(v) => { println!("r={:?}", v); }
                Err(_) => { dbg!("not column"); }
            }
        }
        Err(_) => { dbg!("not exist"); }
    }
    Ok(())
}