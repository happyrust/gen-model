use std::sync::Arc;
use aios_core::pdms_types::RefU64;
use aios_core::prim_geo::tubing::TubiEdgeAql;
use arangors_lite::{AqlQuery, Database};
use bevy::utils::HashMap;
use dashmap::DashMap;
use smol_str::SmolStr;
use sqlx::{Executor, MySql, Pool};
use crate::api::children::travel_children_with_type;
use crate::api::element::query_name;
use crate::aql_api::children::query_travel_children_with_type_aql;
use crate::aql_api::foreign_refnos::query_foreign_refno_aql;
use crate::data_interface::tidb_manager::{AiosDBManager, TUBI_TOL};
use crate::graph_db::pdms_arango::get_arangodb_conn_from_db_option;
use crate::options::DbOption;

/// 找到某个节点下所有的 bran 中的 tubi
pub async fn query_all_tubi_from_node(refno: RefU64, tubi_map: &mut Arc<DashMap<(RefU64, String), f32>>, database: &Database, pool: &Pool<MySql>) -> anyhow::Result<()> {
    let brans = query_travel_children_with_type_aql(database, refno, "BRAN").await?;
    for bran in brans {
        let tubis = query_tubi_from_bran(bran.refno, database).await?;
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
                    query_foreign_refno_aql(from_refno, vec!["HSTU", "HSTU"], database).await?.unwrap_or_default()
                } else {
                    // 如果 tubi 在 bran 的中间或者最后一个，则取上一个节点的 lstu
                    query_foreign_refno_aql(from_refno, vec!["LSTU", "LSTU"], database).await?.unwrap_or_default()
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
pub async fn query_tubi_from_bran(bran_refno: RefU64, database: &Database) -> anyhow::Result<Vec<TubiEdgeAql>> {
    let key = format!("pdms_eles/{}", bran_refno.to_url_refno());
    let aql = AqlQuery::new("
    let bran_name = ( return document('pdms_eles',@bran_refno).name )
    for v,e in 0..100 outbound @id tubi_edges
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
        .bind_var("bran_refno", bran_refno.to_url_refno());
    let mut results: Vec<TubiEdgeAql> = database.aql_query(aql).await?;
    // 过滤不是 tubi 的数据
    results.retain(|r| {
        let distance = r.start_pt.distance(r.end_pt);
        distance > TUBI_TOL
    });
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
