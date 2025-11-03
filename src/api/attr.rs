use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use aios_core::{AttrMap, RefU64Vec};
use aios_core::consts::*;
use aios_core::helper::table::qualified_table_name;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam::{PrimExtrusion, PrimRevolution};
use aios_core::pdms_data::ATTR_INFO_MAP;
use aios_core::pdms_types::*;
use aios_core::tool::db_tool::{db1_dehash, db1_hash};
use anyhow::anyhow;
use sqlx::{Error, MySql, Pool, pool, Row};
use smol_str::SmolStr;
use dashmap::DashMap;
use glam::{Quat, Vec3};
use indexmap::IndexMap;
use itertools::Itertools;
use lazy_static::lazy_static;
use sqlx::Executor;
use sqlx::mysql::MySqlRow;
use crate::api::children::{query_ancestor_of_type_from_cache, query_owner_till_type};
use crate::api::element::{query_ele_node, query_owner_from_id, query_pdms_elements_type_name, query_refno_type, query_types_refnos};
use crate::consts::*;
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_inst_arango::query_insts_shape_data;
use aios_core::AttrVal;


impl AiosDBManager{

    ///获得集几何体的宽度
    pub async fn get_geo_width(&self, refnos: &[RefU64]) -> anyhow::Result<IndexMap<RefU64, f32>>{

        let geos_info_data = query_insts_shape_data(&self.get_arango_db().await?, refnos,  Some(&[GeoBasicType::Pos])).await?;
        //FLOOR WALL STALL PANEL
        let mut res_map = IndexMap::new();
        for refno in refnos {
            let Some(geos_info) = geos_info_data.get_info(refno) else{
                continue;
            };
            let Some(geo_insts) = geos_info_data.get_inst_geos(geos_info) else{
                continue;
            };
            // let pos_geos = geo_insts.iter().filter(|x| x.geo_type == GeoBasicType::Pos ).collect::<Vec<_>>();
            if geo_insts.is_empty() { continue;  }
            let geo_inst = &geo_insts[0];
            let type_name = self.get_type_name(*refno).await;

             match &geo_inst.geo_param {
                PrimRevolution(r) => {
                    use parry2d::bounding_volume::Aabb;
                    let pts = r.verts
                        .iter()
                        .map(|x| nalgebra::Point2::from(nalgebra::Vector2::from(x.truncate())))
                        .collect::<Vec<_>>();
                    let profile_aabb = Aabb::from_points(&pts);
                    let width = profile_aabb.extents().y as f32;
                    res_map.insert(*refno, width);
                }
                PrimExtrusion(e) => {
                    match type_name.as_str() {
                        "FLOOR" | "PANEL" => {
                            res_map.insert(*refno, e.height);
                        }
                        _ =>{
                            use parry2d::bounding_volume::Aabb;
                            let pts = e.verts
                                .iter()
                                .map(|x| nalgebra::Point2::from(nalgebra::Vector2::from(x.truncate())))
                                .collect::<Vec<_>>();
                            let profile_aabb = Aabb::from_points(&pts);
                            let width = profile_aabb.extents().y as f32;
                            res_map.insert(*refno, width);
                        }
                    }
                }
                _ => {}
            }

        }

        Ok(res_map)
    }

}

/// 指定从特定的表查询数据，根据owner查询
pub async fn query_implicit_attrs_by_owner(owner: RefU64, type_name: &str, pool: &Pool<MySql>,
                                           column_names: Option<Vec<&str>>) -> anyhow::Result<Vec<AttrMap>> {
    let sql = gen_query_implicit_attr_sql_by_owner(owner, &type_name, &column_names);
    let column_names = column_names.unwrap_or_default();
    let rows = sqlx::query(&sql).fetch_all(pool).await?;
    let type_hash = db1_hash(type_name.to_uppercase().as_str());

    let mut att_maps = vec![];
    for r in &rows {
        let a = convert_row_to_attmap(r, type_hash as i32, &column_names)?;
        att_maps.push(a);
    }
    Ok(att_maps)
}

#[inline]
pub fn convert_row_to_attmap(row: &MySqlRow, type_hash: i32, column_names: &[&str]) -> anyhow::Result<AttrMap> {
    let mut r = AttrMap::default();
    if let Some(val) = ATTR_INFO_MAP.get(&type_hash) {
        for info in val.value() {
            if !column_names.is_empty() && !column_names.contains(&info.name.as_str()) {
                continue;
            }
            //type 需要获取
            if info.offset != 0 || info.hash as u32 == TYPE_HASH {
                let t = info.name.as_str();
                //todo 需要进一步查找原因
                if t == "DETR" {
                    continue;
                }
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
                            r.entry(hash).or_insert(AttrVal::StringType(v.to_string()))
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

// 已废弃: cache 模块已移除
// /// 获得隐式属性
// pub async fn query_implicit_attr(refno: RefU64, ref_basic: &CachedRefBasic,
//                                  pool: &Pool<MySql>, column_names: Option<Vec<&str>>) -> anyhow::Result<AttrMap> {
pub async fn query_implicit_attr_deprecated(refno: RefU64,
                                 pool: &Pool<MySql>, column_names: Option<Vec<&str>>) -> anyhow::Result<AttrMap> {
    // let type_name = ref_basic.get_type_str();
    // let type_hash = ref_basic.get_noun_hash() as i32;
    // let mut exclude_columns = vec![];
    // //需要过滤一遍
    // let column_names = if column_names.is_some() {
    //     let mut column_names = column_names.unwrap();
    //     if column_names.len() == 0 { return Ok(AttrMap::default()); }
    //     if let Some(names_map) = ATTR_INFO_MAP.get_names_of_type(type_name) {
    //         // exclude_columns = column_names.drain_filter(|x| {
    //         //     !names_map.value().contains(*x)
    //         // }).collect();
    //         let mut i = 0;
    //         while i < column_names.len() {
    //             if !names_map.value().contains(column_names[i]) {
    //                 exclude_columns.push(column_names[i]);
    //                 column_names.swap_remove(i);
    //             } else {
    //                 i += 1;
    //             }
    //         }
    //     }
    //     column_names
    // } else {
    //     vec![]
    // };
    // let sql = gen_query_implicit_attr_sql(refno, ref_basic.get_table_name(), &column_names);
    // let row = sqlx::query(&sql).fetch_one(pool).await?;
    // let mut r = convert_row_to_attmap(&row, type_hash, &column_names);
    // let mut r = r?;
    // //其他的插入
    // if exclude_columns.len() > 0 {
    //     exclude_columns.iter().for_each(|x| {
    //         let hash = NounHash::from(db1_hash(*x));
    //         r.insert(hash, AttrVal::InvalidType);
    //     });
    // }
    // Ok(r)
    Ok(Default::default())
}

/// 查找整张表的 外键 refno 返回自身 refno + foreign refno
pub async fn query_foreign_refnos_from_table(noun: &str, table_name: &str, pool: &Pool<MySql>) -> anyhow::Result<Vec<(RefU64, RefU64)>> {
    let mut r = vec![];
    let sql = gen_query_value_from_table(noun, table_name);
    let results = sqlx::query(&sql).fetch_all(pool).await;
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
    let result = sqlx::query(&sql).fetch_one(pool).await?;
    let val = result.get::<Vec<u8>, _>("DATA");
    Ok(AttrMap::from_compress_bytes(&val).unwrap_or_default())
}

/// 查找该类型对应的所有 uda
pub async fn query_uda_attr(att_type: Vec<i32>, pool: &Pool<MySql>) -> anyhow::Result<AttrMap> {
    let mut map = AttrMap::default();
    let sql = gen_query_uda_attr_sql(att_type);
    let result = sqlx::query(&sql).fetch_all(pool).await;
    if result.is_err() { return Ok(AttrMap::default()); }
    let results = result.unwrap();
    for result in results {
        let val = result.get::<Vec<u8>, _>("DATA");
        let query_map = AttrMap::from_compress_bytes(&val).unwrap_or_default();
        for (k, v) in query_map.map.into_iter() {
            map.map.entry(k).or_insert(v);
        }
    }
    Ok(map)
}

/// 查询该参考号某个uda的值
pub async fn query_refno_uda_value(refno: RefU64, uda_name: &str, pool: &Pool<MySql>) -> anyhow::Result<Option<AttrVal>> {
    let uda_name = if uda_name.starts_with(":") { uda_name[1..].to_string() } else { uda_name.to_string() };
    // 查询 uda 对应的 ukey
    let ukey = query_uda_ukey(&uda_name, pool).await? as u32;
    // 再找到显示属性中对应的值
    let explicit_attr = query_explicit_attr(refno, pool).await?;
    let uda_value = explicit_attr.get(&ukey);
    Ok(uda_value.map(|x| x.clone()))
}

pub async fn query_attr(refno: RefU64, aios_mgr: &AiosDBManager, column_names: Option<Vec<&str>>) -> anyhow::Result<AttrMap> {
    // if let Some((project, pool)) = aios_mgr.get_project_pool_by_refno(refno).await {
    //     let ref_basic = aios_mgr.get_refno_basic(refno);
    //     if ref_basic.is_none() { return Ok(AttrMap::default()); }
    //     let ref_basic = ref_basic.unwrap();
    //     //need to use join
    //     let mut attr = query_implicit_attr(refno, ref_basic.value(), &pool, column_names).await?;
    //     let att_type = attr.get_type_str().to_string();
    //     let explicit_attr = query_explicit_attr(refno, &pool).await?;
    //     let ele = query_ele_node(refno, &pool).await?;
    //     // let b_bran = query_ancestor_of_type_from_cache(ele.refno, "PIPE").is_some();
    //
    //     for (k, v) in explicit_attr.map {
    //         attr.entry(k).or_insert(v);
    //     }
    //
    //     // 赋默认值
    //     if let Some(map) = ATTR_INFO_MAP.map.get(&(db1_hash(&ele.noun) as i32)) {
    //         for values in map.value() {
    //             attr.entry((*values.key() as u32)).or_insert(values.default_val.clone());
    //         }
    //     }
    //     attr.insert(REFNO_HASH, AttrVal::RefU64Type(ele.refno));
    //     attr.insert(NAME_HASH, AttrVal::StringType(ele.name.into()));
    //     attr.insert(OWNER_HASH, AttrVal::RefU64Type(ele.owner));
    //     return Ok(attr);
    // }
    Ok(AttrMap::default())
}

pub async fn insert_attr_info(pool: Pool<MySql>) -> anyhow::Result<()> {
    let sql = gen_insert_attr_info_sql(&ATTR_INFO_MAP);
    let mut conn = pool;
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

pub async fn query_position_from_id(refno: RefU64, aios_mgr: &AiosDBManager) -> anyhow::Result<Option<Vec3>> {
    // let type_name = query_refno_type(refno, pool).await?;
    let table_name = aios_mgr.get_refno_basic(refno);
    if table_name.is_none() { return Ok(None); }
    let table_name = table_name.unwrap();
    let pool = aios_mgr.get_project_pool_by_refno(refno).await;
    if pool.is_none() { return Ok(None); }
    let (_, pool) = pool.unwrap();
    let sql = gen_position_from_id(refno, &table_name.value().table);
    let result = sqlx::query(&sql).fetch_one(&pool).await;
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
    let result = sqlx::query(&sql).fetch_one(pool).await;
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
    let result = sqlx::query(&sql).fetch_one(pool).await;
    return match result {
        Ok(v) => {
            return Ok(Some(RefU64(v.get::<i64, _>(0) as u64)));
        }
        Err(_) => { Ok(None) }
    };
}

pub async fn query_numbdbs_by_mdb(dbs: RefU64Vec, pool: &Pool<MySql>) -> anyhow::Result<Vec<u32>> {
    let mut r = vec![];
    let sql = gen_query_numbdbs_by_mdb_sql(dbs);
    let results = sqlx::query(&sql).fetch_all(pool).await;
    if let Ok(results) = results {
        for result in results {
            let numbdb = result.get::<i32, _>("NUMBDB");
            r.push(numbdb as u32);
        }
    }
    Ok(r)
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
pub fn gen_query_implicit_attr_sql(refno: RefU64, table_name: &str, columns: &[&str]) -> String {
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

/// 获取site属于哪个专业
pub async fn get_site_major_from_uda(site_refno: RefU64, pool: &Pool<MySql>) -> Option<UdaMajorType> {
    if let Ok(explicit_attr) = query_explicit_attr(site_refno, &pool).await {
        if let Some(major) = explicit_attr.map.get(&(688051936)) {
            let major_str = major.string_value();
            return Some(UdaMajorType::from_str(major_str.as_str()));
        }
    }
    None
}

/// 获取该项目对应的所有的 uda name ，并过滤调 udna 为空的情况
pub async fn query_uda_ukey_udna_all(pool: &Pool<MySql>) -> anyhow::Result<HashMap<u32, String>> {
    let mut result = HashMap::new();
    let sql = gen_query_uda_name_sql();
    let query_results = sqlx::query(&sql).fetch_all(pool).await?;
    for query_result in query_results {
        let u_key = query_result.get::<i32, _>("UKEY");
        let u_name = query_result.get::<String, _>("UDNA");
        result.entry(u_key as u32).or_insert(u_name);
    }
    Ok(result)
}

/// 查找所有的自定义类型的udna以及ukey
pub async fn query_uda_ukey_udet_all(pool: &Pool<MySql>) -> anyhow::Result<HashMap<u32, String>> {
    let mut result = HashMap::new();
    let sql = gen_query_udet_name_sql();
    let query_results = sqlx::query(&sql).fetch_all(pool).await?;
    for query_result in query_results {
        let u_key = query_result.get::<i32, _>("UKEY");
        let u_name = query_result.get::<String, _>("UDNA");
        result.entry(u_key as u32).or_insert(u_name);
    }
    Ok(result)
}

/// 查找某个uda对应的ukey
pub async fn query_uda_ukey(uda: &str, pool: &Pool<MySql>) -> anyhow::Result<i32> {
    let sql = gen_query_uda_ukey_sql(uda);
    let query_result = sqlx::query(&sql).fetch_one(pool).await?;
    let u_key = query_result.get::<i32, _>("UKEY");
    Ok(u_key)
}

pub fn gen_query_explicit_attr_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT DATA FROM {PDMS_EXPLICIT_TABLE} WHERE ID = {} ;", refno.0));
    sql
}

pub fn gen_query_uda_attr_sql(att_types: Vec<i32>) -> String {
    let mut sql = String::new();
    let mut types = String::new();
    let is_empty = att_types.is_empty();
    for att_type in att_types {
        types.push_str(&format!("{} ,", att_type));
    }
    if !is_empty {
        types.remove(types.len() - 1);
    }
    sql.push_str(&format!("SELECT TYPE,DATA FROM {PDMS_UDA_ATT_TABLE} WHERE TYPE IN ({});", types));
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

fn gen_query_numbdbs_by_mdb_sql(dbs: RefU64Vec) -> String {
    let mut dbs_sql = String::new();
    let b_empty = dbs.0.is_empty();
    for db in dbs {
        dbs_sql.push_str(&format!("{} ,", db.0));
    }
    if !b_empty { dbs_sql.remove(dbs_sql.len() - 1); }
    let mut sql = String::new();
    sql.push_str(&format!("SELECT NUMBDB FROM DB WHERE ID IN ( {} )", dbs_sql));
    sql
}

fn gen_query_uda_name_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT UKEY,UDNA FROM {PDMS_UDA_TABLE} WHERE UDNA != ''"));
    sql
}

fn gen_query_udet_name_sql() -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT UKEY,UDNA FROM {PDMS_UDET_TABLE} WHERE UDNA != ''"));
    sql
}

fn gen_query_uda_ukey_sql(uda: &str) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT UKEY,UDNA FROM {PDMS_UDA_TABLE} WHERE UDNA = '{}'", uda));
    sql
}

#[tokio::test]
async fn test_query_foreign_refno() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "AvevaMarineSample").await?;
    let refno: RefU64 = RefI32Tuple((24575, 2178)).into();
    let v = query_explicit_attr(refno, &pool).await?;
    println!("v={:?}", v);
    Ok(())
}

#[tokio::test]
async fn test_query_refno_uda_value() -> anyhow::Result<()> {
    let _ = dotenv::dotenv();
    let url = env::var("DATABASE_URL")?;
    let pool = AiosDBManager::get_db_pool(&url, "AvevaMarineSample").await?;
    let refno: RefU64 = RefI32Tuple((17496,124126)).into();
    let result = query_refno_uda_value(refno,":JGOBJBASE",&pool).await?;
    dbg!(&result);
    Ok(())
}
