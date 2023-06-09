use std::sync::Arc;
use aios_core::data_center::TubiData;
// use aios_core::data_center::TubiData;
use aios_core::pdms_types::RefU64;
use aios_core::prim_geo::tubing::TubiEdge;
use bb8_arangodb::arangors::{AqlQuery, Database};
use bevy::prelude::{dbg, unwrap};
use dashmap::DashMap;
use glam::Vec3;


use smol_str::SmolStr;
use sqlx::{Executor, MySql, Pool};
use crate::api::children::travel_children_with_type;
use crate::api::element::query_name;
use crate::aql_api::children::query_travel_children_with_type_aql;
use crate::aql_api::foreign_refnos::{query_foreign_name_aql, query_foreign_refno_aql};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::graph_db::pdms_arango::ArDatabase;
use crate::pcf::bran::get_bran_name_and_children;
use crate::pcf::excel_api::get_pipe_thickness_table;
use crate::consts::AQL_PDMS_ELES_COLLECTION;
use crate::data_interface::db_model::TUBI_TOL;

/// 找到某个节点下所有的 bran 中的 tubi
pub async fn query_all_tubi_from_node(refno: RefU64, tubi_map: &mut Arc<DashMap<(RefU64, String), f32>>, database: &ArDatabase, pool: &Pool<MySql>) -> anyhow::Result<()> {
    let brans = query_travel_children_with_type_aql(database, refno, "BRAN").await?;
    for bran in brans {
        let tubis = query_tubi_from_bran(bran.refno, &database).await?;
        for tubi in tubis {
            let distance = tubi.start_pt.distance(tubi.end_pt);
            // 符合 tubi 条件
            if distance >= TUBI_TOL {
                let from_refno = tubi._from.split("/").collect::<Vec<_>>();
                if from_refno.is_empty() { continue; }
                let from_refno = RefU64::from_url_refno(from_refno.last().unwrap());
                if from_refno.is_none() { continue; }
                let from_refno = from_refno.unwrap();
                // 如果是 tubi 在 bran 的第一个，取 bran 的 hstu
                let spre = if from_refno == bran.refno {
                    query_foreign_refno_aql(from_refno, &["HSTU", "HSTU"], &database).await?.unwrap_or_default()
                } else {
                    // 如果 tubi 在 bran 的中间或者最后一个，则取上一个节点的 lstu
                    query_foreign_refno_aql(from_refno, &["LSTU", "LSTU"], &database).await?.unwrap_or_default()
                };
                let spre_name = if spre == RefU64(0) {
                    "0/0".to_string()
                } else {
                    let name = query_name(spre, pool).await.unwrap_or("0/0".to_string());
                    if name.starts_with('/') { name[1..].to_string() } else { name }
                };
                *tubi_map.entry((spre, spre_name)).or_insert(0.0) += distance;
            }
        }
    }
    Ok(())
}

/// 找到 bran 下所有的 tubi
pub async fn query_tubi_from_bran(bran_refno: RefU64, database: &ArDatabase) -> anyhow::Result<Vec<TubiEdge>> {
    let key = format!("{AQL_PDMS_ELES_COLLECTION}/{}", bran_refno.to_url_refno());
    let aql = AqlQuery::builder().query("
    let bran_name = ( return document('pdms_eles',@bran_refno).name )
    for v,e in 0..1000 outbound @id tubi_edges
    filter bran_name[0] != null
    filter bran_name[0] == e.bran_name
    filter e != null
    return {
        '_key': e._key,
        '_from': e._from,
        '_to':e._to,
        'start_pt': e.start_pt,
        'end_pt': e.end_pt,
        'att_type': e.att_type,
        'bran_name': e.bran_name,
        'extra_type': e.extra_type,
        'bore': e.bore
    }")
        .bind_var("id", key)
        .bind_var("bran_refno", bran_refno.to_url_refno())
        .build();
    let mut results: Vec<TubiEdge> = database.aql_query(aql).await?;
    // 过滤不是 tubi 的数据
    results.retain(|r| {
        let distance = r.start_pt.distance(r.end_pt);
        distance > TUBI_TOL
    });
    Ok(results)
}

/// 找到 bran 下所有的 tubi ，并过滤掉 atta
pub async fn query_tubi_from_bran_filter_atta(bran_refno: RefU64, database: &ArDatabase) -> anyhow::Result<Vec<TubiEdge>> {
    let mut tubi = Vec::new();
    let key = format!("{AQL_PDMS_ELES_COLLECTION}/{}", bran_refno.to_url_refno());
    let aql = AqlQuery::builder().query("
    let bran_name = ( return document('pdms_eles',@bran_refno).name )
    for v,e in 0..1000 outbound @id tubi_edges
    filter bran_name[0] != null
    filter bran_name[0] == e.bran_name
    filter e != null
    return {
        '_key': e._key,
        '_from': e._from,
        '_to':e._to,
        'start_pt': e.start_pt,
        'end_pt': e.end_pt,
        'att_type': e.att_type,
        'bran_name': e.bran_name,
        'extra_type': e.extra_type,
        'bore': e.bore
    }")
        .bind_var("id", key)
        .bind_var("bran_refno", bran_refno.to_url_refno())
        .build();
    let results: Vec<TubiEdge> = database.aql_query(aql).await?;
    // 过滤 atta
    let mut i = 0;
    while i < results.len() {
        let distance = results[i].start_pt.distance(results[i].end_pt);
        if distance >= TUBI_TOL {
            // atta 跳过 继续往后找到下一个非 atta 元件
            if results[i].att_type.to_uppercase().as_str() == "ATTA" {
                let mut j = i;
                while j < results.len() && results[j].att_type.to_uppercase().as_str() == "ATTA" {
                    j += 1;
                    if j < results.len() && (results[j].att_type.to_uppercase().as_str() != "ATTA" || j == results.len() - 1) {
                        tubi.push(TubiEdge {
                            _key: results[i]._key.to_string(),
                            _from: results[i]._from.to_string(),
                            _to: results[j]._to.to_string(),
                            start_pt: results[i].start_pt,
                            end_pt: results[j].end_pt,
                            att_type: results[j].att_type.to_string(),
                            extra_type: results[j].extra_type.to_string(),
                            bore: results[j].bore,
                            bran_name: results[i].bran_name.to_string(),
                        });
                        i = j;
                        break;
                    }
                }
            } else {
                tubi.push(TubiEdge {
                    _key: results[i]._key.to_string(),
                    _from: results[i]._from.to_string(),
                    _to: results[i]._to.to_string(),
                    start_pt: results[i].start_pt,
                    end_pt: results[i].end_pt,
                    att_type: results[i].att_type.to_string(),
                    extra_type: results[i].extra_type.to_string(),
                    bore: results[i].bore,
                    bran_name: results[i].bran_name.to_string(),
                });
            }
        }
        i += 1;
    }
    Ok(tubi)
}

//
// #[tokio::test]
// pub async fn test_() -> anyhow::Result<()> {
//     let mut mgr = AiosDBManager::init_form_config().await;
//     let database = mgr.as_ref().expect("REASON").get_arangodb_conn().await.unwrap().clone();
//     let mut pos_vec = Vec::new();
//     let refno = RefU64::from_refno_str("24381/147719").unwrap();
//     // 取arrive，leave
//     let data = query_bran_info(refno, &database).await.unwrap();
//     //取hpos,取tpos
//     let len = data.len();
//     let hpos = data[0].start_pt;
//     let tpos = data[len - 1].end_pt;
//     pos_vec.push(hpos);
//     // 取wrt
//     for i in data {
//         //获取转折点坐标
//         if i.att_type == "ELBO" || i.att_type == "BEND" {
//             let refno: Vec<&str> = i._to.split("/").collect();
//             let refno = refno[1];
//             let result = mgr.as_ref().unwrap().get_world_transform(RefU64::from_url_refno(refno).unwrap()).await.unwrap().unwrap().clone();
//             pos_vec.push(result.translation);
//         }
//     }
//     pos_vec.push(tpos);
//     let mut dis_vec = Vec::new();
// //求每段直段的距离
//     for i in 0..(pos_vec.len() - 1) {
//         let dx = pos_vec[i + 1].x - pos_vec[i].x;
//         let dy = pos_vec[i + 1].y - pos_vec[i].y;
//         let dz = pos_vec[i + 1].z - pos_vec[i].z;
//         dis_vec.push((dx.powi(2) + dy.powi(2) + dz.powi(2)).sqrt().round());
//     }
//     //第一个ATTA点在500mm处，最后一个ATTA点离TPOS100mm以上，中间每间隔interval设置一个ATTA
//     let mut atta_vec = Vec::new();
//     //当前在哪段
//     let mut index = 0;
//     let mut dis = dis_vec[index];
//     let interval = 5500.0;
//     if dis >= 500.0 {
//         let pos = atta_pos(pos_vec[index], pos_vec[index + 1], 500.0);
//         atta_vec.push(pos);
//         dis = dis - 500.0;
//     }
//     while index < (pos_vec.len() - 2) || dis >= interval {
//         if dis >= interval {
//             dis -= interval;
//             let pos = atta_pos(pos_vec[index], pos_vec[index + 1], interval);
//             pos_vec.push(pos);
//         } else {
//             index += 1;
//             dis += dis_vec[index];
//         }
//     }
//     dbg!(pos_vec);
//     Ok(())
// }

// pub fn atta_pos(s_pos: Vec3, e_pos: Vec3, distance: f32) -> Vec3 {
//     let x1 = s_pos.x;
//     let y1 = s_pos.y;
//     let z1 = s_pos.z;
//     let x2 = e_pos.x;
//     let y2 = e_pos.y;
//     let z2 = e_pos.z;
//     let dx = x2 - x1;
//     let dy = y2 - y1;
//     let dz = z2 - z1;
//     let line_length = ((dx * dx + dy * dy + dz * dz).sqrt());
//     let ratio = distance / line_length;
//     return Vec3::new(x1 + ratio * dx, y1 + ratio * dy, z1 + ratio * dz);
// }


/// 获取 bran 所有的 tubi_edge 的信息
pub async fn query_bran_info(bran_refno: RefU64, database: &ArDatabase) -> anyhow::Result<Vec<TubiEdge>> {
    let key = format!("{AQL_PDMS_ELES_COLLECTION}/{}", bran_refno.to_url_refno());
    let aql = AqlQuery::builder().query("
    let bran_name = ( return document('pdms_eles',@bran_refno).name )
    for v,e in 0..1000 outbound @id tubi_edges
    filter bran_name[0] != null
    filter bran_name[0] == e.bran_name
    filter e != null
    return {
        '_key': e._key,
        '_from': e._from,
        '_to':e._to,
        'start_pt': e.start_pt,
        'end_pt': e.end_pt,
        'att_type': e.att_type,
        'bran_name': e.bran_name,
        'extra_type': e.extra_type,
        'bore': e.bore
    }")
        .bind_var("id", key)
        .bind_var("bran_refno", bran_refno.to_url_refno())
        .build();
    let results: Vec<TubiEdge> = database.aql_query(aql).await?;
    Ok(results)
}

pub async fn insert_tubi_value(tubi_map: DashMap<(RefU64, String), f32>, pool: &Pool<MySql>) -> anyhow::Result<()> {
    let mut sql = "INSERT INTO `工艺布置专业_大宗材料`(`参考号`,`编码`,`类型`,`长度`) VALUES ".to_string();
    let b_empty = tubi_map.is_empty();
    for tubi in tubi_map.into_iter() {
        let refno = tubi.0.0;
        let spre_name = tubi.0.1;
        sql.push_str(&format!("( '{}' ,'{}','TUBI','{}' ),", refno.to_refno_str(), spre_name, tubi.1.to_string()));
    }
    if !b_empty {
        sql.remove(sql.len() - 1);
        let r = pool.execute(sql.as_str()).await;
        if let Err(err) = r {
            dbg!(&sql);
            dbg!(&err);
        }
    }
    Ok(())
}

/// 找到bran里面所有的tubi，并过滤掉 atta ，找到tubi对应的长度和lstu（第一个元素为hstu）
pub async fn query_tubi_lstu(bran_refno: RefU64, database: &ArDatabase) -> anyhow::Result<Vec<TubiData>> {
    let mut result = Vec::new();
    let tubis = query_tubi_from_bran_filter_atta(bran_refno, &database).await?;
    for tubi in tubis {
        let from_refno = RefU64::from_arangodb_refno_str(&tubi._from);
        if from_refno.is_none() { continue; }
        let from_refno = from_refno.unwrap();
        // bran下面的 tubi应该取hstu
        let lstu = if bran_refno == from_refno {
            query_foreign_name_aql(from_refno, vec!["HSTU", "HSTU"], &database).await?
        } else {
            query_foreign_name_aql(from_refno, vec!["LSTU", "LSTU"], &database).await?
        };
        if let Some(lstu) = lstu {
            result.push(TubiData {
                pre_refno: from_refno,
                lstu_name: lstu,
                length: tubi.start_pt.distance(tubi.end_pt),
            });
        }
    }
    Ok(result)
}

#[tokio::test]
async fn test_query_tubi_from_bran_filter_atta() -> anyhow::Result<()> {
    // use config::{Config, ConfigError, Environment, File};
    // let s = Config::builder()
    //     .add_source(File::with_name("DbOption"))
    //     .build()?;
    // let db_option: DbOption = s.try_deserialize().unwrap();
    // let database = get_arangodb_conn_from_db_option(&db_option).await?;
    // let refno = RefU64::from_refno_str("23584/5443").unwrap();
    // let results = query_tubi_lstu(refno, &database).await?;
    // dbg!(&results);
    Ok(())
}