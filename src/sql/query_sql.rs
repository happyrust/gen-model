use aios_core::pdms_types::{AttrInfo, AttrMap, AttrVal, DbAttributeType, NounHash, RefU64};
use anyhow::anyhow;
use dashmap::DashMap;
use parse_pdms_db::db1_dehash;
use parse_pdms_db::db_tool::db1_hash;
use smol_str::SmolStr;
use sqlx::{MySql, Pool, Row};
use crate::database::get_tidb_pool;
use crate::query_sql::query_refno_infos;
use crate::REFNO_INFO_MAP;
use crate::sql::gen_sql::gen_query_implicit_attr_sql;

pub async fn query_implicit_attr(refno: RefU64, type_name: &str, pool: Pool<MySql>) -> anyhow::Result<AttrMap> {
    let mut r = AttrMap::default();
    let type_hash = db1_hash(type_name);
    let sql = gen_query_implicit_attr_sql(refno, type_name);
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

#[tokio::test]
async fn test_query_implicit_attr() -> anyhow::Result<()> {
    let url = "mysql://root:root@127.0.0.1:3306";
    let info_pool = get_tidb_pool(&format!("{}/{}", url, "refno_infos")).await;
    let refno = RefU64(103010495627266);
    let project = query_refno_infos(refno, info_pool).await?;
    let pool = get_tidb_pool(&format!("{}/{}", url, project)).await;
    let v = query_implicit_attr(refno, "SECT", pool).await.unwrap();
    println!("v={:?}", v.to_string_hashmap());
    Ok(())
}