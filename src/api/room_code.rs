use aios_core::pdms_types::*;

use sqlx::{MySql, Pool, Row};
use crate::api::element::query_ele_nodes_by_refnos;
use crate::aql_api::convert_refno_vec_from_vec_string;
use crate::aql_api::pdms_room::query_all_need_compute_room_refno;

// 查询房间号
pub async fn query_room_code(refno: RefU64, pool: &Pool<MySql>) -> anyhow::Result<Option<String>> {
    let sql = gen_query_room_code_sql(refno);
    let result = sqlx::query(&sql).fetch_one(pool).await;
    return match result {
        Ok(val) => {
            Ok(Some(val.get::<String, _>("ROOM_NAME")))
        }
        Err(e) => {
            Ok(None)
        }
    };
}

// 查询多个参考号所在的房间号
pub async fn query_room_code_with_refnos(refnos: Vec<RefU64>, pool: &Pool<MySql>) -> anyhow::Result<Vec<(RefU64, String)>> {
    let mut result = Vec::new();
    let sql = gen_query_room_code_with_refnos_sql(refnos);
    let query_result = sqlx::query(&sql).fetch_all(pool).await;
    match query_result {
        Ok(vals) => {
            for val in vals {
                let id = val.get::<i64, _>("ID");
                let room_name = val.get::<String, _>("ROOM_NAME");
                result.push((RefU64(id as u64), room_name))
            }
        }
        Err(e) => {
            return Ok(vec![]);
        }
    };
    Ok(result)
}

// 查找所有房间节点，暂时按 1516 命名格式过滤
pub async fn query_room_nodes(dbno: &Vec<i32>, pool: &Pool<MySql>) -> anyhow::Result<Vec<PdmsElement>> {
    let room_infos = query_all_need_compute_room_refno(
        dbno,
        "FRMW",
        Some("-RM"),
        pool,
    ).await?;
    let room_infos = room_infos.into_iter().map(|x| x.0).collect::<Vec<_>>();
    let nodes = query_ele_nodes_by_refnos(room_infos, pool).await?;
    Ok(nodes.into_iter().map(|x| x.into()).collect())
}

fn gen_query_room_code_sql(refno: RefU64) -> String {
    let mut sql = String::new();
    sql.push_str(&format!("SELECT ROOM_NAME FROM ROOM_CODE WHERE REFNO = {}", refno.0));
    sql
}

fn gen_query_room_code_with_refnos_sql(refnos: Vec<RefU64>) -> String {
    let mut sql = String::new();
    let mut refno_str = String::new();
    let is_empty = refnos.is_empty();
    for refno in refnos {
        refno_str.push_str(&format!("{} ,", refno.to_string()));
    }
    if !is_empty {
        refno_str.remove(refno_str.len() - 1);
    }
    sql.push_str(&format!("SELECT REFNO,ROOM_NAME FROM ROOM_CODE WHERE REFNO IN ( {} )", refno_str));
    sql
}