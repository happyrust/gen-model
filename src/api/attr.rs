use aios_core::pdms_types::{AttrMap, AttrVal, DbAttributeType, NounHash, RefU64};
use sqlx::{MySql, Pool, Row};
use parse_pdms_db::db_tool::db1_hash;
use parse_pdms_db::db1_dehash;
use smol_str::SmolStr;
use crate::query_sql::query_pdms_elements_type_name;
use crate::REFNO_INFO_MAP;
use crate::sql::gen_sql::gen_query_implicit_attr_sql;

pub async fn query_implicit_attr(refno: RefU64, pool: Pool<MySql>) -> anyhow::Result<AttrMap> {
    let mut r = AttrMap::default();
    let type_name = query_pdms_elements_type_name(refno,pool.clone()).await?;
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
